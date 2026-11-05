//! PCM cache for lossy formats (ADR-0008).
//!
//! Lossy files are decoded **once at load** into a cache entry and played from there: no decoder
//! in the playback path, exact sample positions, gapless trimming done once. Lossless formats never
//! come here — they stream with an index.
//!
//! Two rules keep the cache honest:
//!
//! * the key is derived from the **file content**, not from the path, so a replaced file never
//!   plays stale audio;
//! * the cache is always regenerable: the source file remains the only truth, and a missing or
//!   unreadable entry is simply rebuilt.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::decode::StreamDecoder;
use crate::info::{MediaError, MediaFormat, MediaInfo};

/// Magic of the cache entry file.
const MAGIC: &[u8; 8] = b"DJPCM\0\0\0";
/// Version of the entry format; bump when the layout changes so old entries are ignored.
pub const CACHE_FORMAT_VERSION: u32 = 1;
/// Where the cache lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStorage {
    /// Files under `$XDG_CACHE_HOME/hauler-dj/pcm` (default: survives restarts, costs disk).
    Disk,
    /// Cached in RAM for the life of the process.
    Memory,
}

/// Stored sample format of a cache entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmSampleFormat {
    /// 16-bit signed, 2 bytes per sample.
    I16,
    /// 24-bit signed packed in 3 bytes.
    I24,
    /// 32-bit float.
    F32,
}

impl PcmSampleFormat {
    /// Bytes per sample.
    pub fn bytes_per_sample(self) -> u64 {
        match self {
            PcmSampleFormat::I16 => 2,
            PcmSampleFormat::I24 => 3,
            PcmSampleFormat::F32 => 4,
        }
    }

    /// Stable code stored in the entry header.
    pub fn code(self) -> u16 {
        match self {
            PcmSampleFormat::I16 => 1,
            PcmSampleFormat::I24 => 2,
            PcmSampleFormat::F32 => 3,
        }
    }

    /// Inverse of [`PcmSampleFormat::code`].
    pub fn from_code(code: u16) -> Option<Self> {
        match code {
            1 => Some(PcmSampleFormat::I16),
            2 => Some(PcmSampleFormat::I24),
            3 => Some(PcmSampleFormat::F32),
            _ => None,
        }
    }

    fn encode(self, sample: f32, out: &mut Vec<u8>) {
        let clamped = sample.clamp(-1.0, 1.0);
        match self {
            PcmSampleFormat::I16 => {
                let value = (clamped * 32_767.0).round() as i16;
                out.extend_from_slice(&value.to_le_bytes());
            }
            PcmSampleFormat::I24 => {
                let value = (clamped * 8_388_607.0).round() as i32;
                let bytes = value.to_le_bytes();
                out.extend_from_slice(&bytes[..3]);
            }
            PcmSampleFormat::F32 => out.extend_from_slice(&clamped.to_le_bytes()),
        }
    }

    fn decode(self, bytes: &[u8]) -> f32 {
        match self {
            PcmSampleFormat::I16 => {
                let value = i16::from_le_bytes([bytes[0], bytes[1]]);
                f32::from(value) / 32_767.0
            }
            PcmSampleFormat::I24 => {
                // Three bytes of two's complement: sign-extend from the top bit of the third.
                let sign = if bytes[2] & 0x80 != 0 { 0xFFu8 } else { 0x00u8 };
                let value = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], sign]);
                value as f32 / 8_388_607.0
            }
            PcmSampleFormat::F32 => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        }
    }
}

/// Cache policy knobs, all of them user settings (DJP-ENV-006).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheConfig {
    /// Cache lossy formats at all.
    pub enabled: bool,
    /// Disk or memory.
    pub storage: CacheStorage,
    /// Hard size limit; entries beyond it are evicted least-recently-used first.
    pub max_bytes: u64,
    /// How samples are stored inside an entry.
    pub sample_format: PcmSampleFormat,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            storage: CacheStorage::Disk,
            max_bytes: 8 * 1_000_000_000,
            sample_format: PcmSampleFormat::I16,
        }
    }
}

/// What the cache did for one load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOutcome {
    /// An existing entry was reused.
    Hit,
    /// The file was decoded into a new entry.
    Built,
    /// The format does not use the cache (lossless).
    NotNeeded,
    /// The cache is disabled by configuration.
    Disabled,
}

/// Where a deck should read a given file from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackSource {
    /// Lossless: stream from the file, seek with the index.
    Streaming {
        /// Classified format of the file.
        format: MediaFormat,
    },
    /// Lossy: decode into the PCM cache at load, then play from the entry.
    PcmCache {
        /// Classified format of the file.
        format: MediaFormat,
    },
}

/// Decide the playback strategy from the file alone (ADR-0008: never from the moment).
pub fn playback_source(info: &MediaInfo, config: &CacheConfig) -> PlaybackSource {
    if info.format.needs_pcm_cache() && config.enabled {
        PlaybackSource::PcmCache {
            format: info.format,
        }
    } else {
        PlaybackSource::Streaming {
            format: info.format,
        }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    key: String,
    bytes: u64,
    last_used_ms: u64,
    last_used_seq: u64,
    pinned: u32,
}

/// A bounded, least-recently-used cache of decoded PCM.
#[derive(Debug)]
pub struct PcmCache {
    root: PathBuf,
    config: CacheConfig,
    entries: Vec<CacheEntry>,
    sequence: u64,
    memory: std::collections::HashMap<String, MemoryEntry>,
}

#[derive(Debug, Clone)]
struct MemoryEntry {
    data: std::sync::Arc<Vec<u8>>,
    sample_rate: u32,
    channels: u16,
    format: PcmSampleFormat,
}

impl PcmCache {
    /// Create a cache rooted at `root` (created if missing). Memory storage ignores the root.
    pub fn new(root: PathBuf, config: CacheConfig) -> Result<Self, MediaError> {
        if config.storage == CacheStorage::Disk {
            std::fs::create_dir_all(&root)?;
        }
        let mut cache = Self {
            root,
            config,
            entries: Vec::new(),
            sequence: 0,
            memory: std::collections::HashMap::new(),
        };
        cache.scan();
        Ok(cache)
    }

    /// Configuration in force.
    pub fn config(&self) -> &CacheConfig {
        &self.config
    }

    /// Total bytes currently held.
    pub fn size_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.bytes).sum()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Key of a file: content hash plus everything that changes the stored samples.
    pub fn key_for(
        path: &Path,
        info: &MediaInfo,
        config: &CacheConfig,
    ) -> Result<String, MediaError> {
        let mut hasher = Sha256::new();
        hasher.update(b"dj-pcm-cache");
        hasher.update(&CACHE_FORMAT_VERSION.to_le_bytes());
        hasher.update(&info.sample_rate.to_le_bytes());
        hasher.update(&info.channels.to_le_bytes());
        hasher.update(&config.sample_format.code().to_le_bytes());
        let mut file = BufReader::new(File::open(path)?);
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(hex(&hasher.finish()))
    }

    /// Make sure an entry exists for `path`, building it when needed.
    pub fn prepare(&mut self, path: &Path) -> Result<CacheOutcome, MediaError> {
        if !self.config.enabled {
            return Ok(CacheOutcome::Disabled);
        }
        let mut decoder = StreamDecoder::open(path)?;
        let info = decoder.info().clone();
        if !info.format.needs_pcm_cache() {
            return Ok(CacheOutcome::NotNeeded);
        }
        let key = Self::key_for(path, &info, &self.config)?;

        if self.touch(&key) {
            return Ok(CacheOutcome::Hit);
        }

        let (frames, data) = self.decode_all(&mut decoder)?;
        let bytes = data.len() as u64;

        match self.config.storage {
            CacheStorage::Memory => {
                self.memory.insert(
                    key.clone(),
                    MemoryEntry {
                        data: std::sync::Arc::new(data),
                        sample_rate: info.sample_rate,
                        channels: info.channels,
                        format: self.config.sample_format,
                    },
                );
            }
            CacheStorage::Disk => {
                let part = self.root.join(format!("{key}.part"));
                let final_path = self.root.join(format!("{key}.pcm"));
                {
                    let file = File::create(&part)?;
                    let mut writer = BufWriter::new(file);
                    write_header(&mut writer, &info, self.config.sample_format, frames)?;
                    writer.write_all(&data)?;
                    writer.flush()?;
                }
                std::fs::rename(&part, &final_path)?;
            }
        }

        self.sequence += 1;
        self.entries.push(CacheEntry {
            key,
            bytes,
            last_used_ms: now_ms(),
            last_used_seq: self.sequence,
            pinned: 0,
        });
        self.enforce_limit()?;
        Ok(CacheOutcome::Built)
    }

    /// Frames of a prepared entry, or `None` when it is gone.
    pub fn frames(&self, key: &str) -> Option<u64> {
        match self.config.storage {
            CacheStorage::Memory => self.memory.get(key).map(|entry| {
                entry.data.len() as u64 / (2 * entry.format.bytes_per_sample()).max(1)
            }),
            CacheStorage::Disk => {
                let file = File::open(self.root.join(format!("{key}.pcm"))).ok()?;
                let mut reader = BufReader::new(file);
                read_header(&mut reader).ok().map(|header| header.frames)
            }
        }
    }

    /// Open a reader over a prepared entry.
    pub fn open_reader(&self, key: &str) -> Result<CachedReader, MediaError> {
        match self.config.storage {
            CacheStorage::Disk => CachedReader::open_disk(&self.root.join(format!("{key}.pcm"))),
            CacheStorage::Memory => {
                let entry = self.memory.get(key).ok_or_else(|| {
                    MediaError::Decode(format!("cache entry {key} is no longer in memory"))
                })?;
                Ok(CachedReader::open_memory(
                    std::sync::Arc::clone(&entry.data),
                    entry.sample_rate,
                    entry.channels,
                    entry.format,
                ))
            }
        }
    }

    /// Pin an entry so eviction skips it (loaded decks).
    pub fn pin(&mut self, key: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) {
            entry.pinned = entry.pinned.saturating_add(1);
        }
    }

    /// Release a pin. Unpinned entries become eviction candidates again.
    pub fn unpin(&mut self, key: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) {
            entry.pinned = entry.pinned.saturating_sub(1);
        }
    }

    /// Drop entries until the cache fits `max_bytes`, least recently used first.
    pub fn enforce_limit(&mut self) -> Result<u64, MediaError> {
        let mut freed = 0u64;
        while self.size_bytes() > self.config.max_bytes {
            let candidate = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.pinned == 0)
                .min_by_key(|(_, entry)| (entry.last_used_seq, entry.last_used_ms))
                .map(|(index, _)| index);
            let Some(index) = candidate else {
                break;
            };
            let entry = self.entries.remove(index);
            freed += entry.bytes;
            self.memory.remove(&entry.key);
            if self.config.storage == CacheStorage::Disk {
                let _ = std::fs::remove_file(self.root.join(format!("{}.pcm", entry.key)));
            }
        }
        Ok(freed)
    }

    fn touch(&mut self, key: &str) -> bool {
        let Some(position) = self.entries.iter().position(|entry| entry.key == key) else {
            return false;
        };
        self.sequence += 1;
        let sequence = self.sequence;
        if let Some(entry) = self.entries.get_mut(position) {
            entry.last_used_ms = now_ms();
            entry.last_used_seq = sequence;
        }
        true
    }

    fn scan(&mut self) {
        if self.config.storage != CacheStorage::Disk {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("pcm") {
                continue;
            }
            let Ok(file) = File::open(&path) else {
                continue;
            };
            let mut reader = BufReader::new(file);
            let Ok(header) = read_header(&mut reader) else {
                let _ = std::fs::remove_file(&path);
                continue;
            };
            let Some(key) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            self.sequence += 1;
            self.entries.push(CacheEntry {
                key: key.to_string(),
                bytes: header.data_bytes,
                last_used_ms: header.written_ms,
                last_used_seq: self.sequence,
                pinned: 0,
            });
        }
    }

    fn decode_all(&self, decoder: &mut StreamDecoder) -> Result<(u64, Vec<u8>), MediaError> {
        let format = self.config.sample_format;
        let mut frames = 0u64;
        let mut data = Vec::new();
        let mut block = vec![0.0f32; 4096 * 2];
        loop {
            let read = decoder.read_frames(&mut block)?;
            if read == 0 {
                break;
            }
            frames += read as u64;
            data.reserve(read * 2 * format.bytes_per_sample() as usize);
            for sample in &block[..read * 2] {
                format.encode(*sample, &mut data);
            }
        }
        Ok((frames, data))
    }
}

/// Frame reader over a cache entry. No decoder, no policy: bytes and a cursor.
pub struct CachedReader {
    backing: Backing,
    format: PcmSampleFormat,
    info: MediaInfo,
    frames: u64,
    scratch: Vec<u8>,
    cursor: u64,
    position: u64,
}

enum Backing {
    /// Entry file on disk; the header has already been consumed.
    Disk(File),
    /// Entry held in RAM.
    Memory(std::sync::Arc<Vec<u8>>),
}

impl std::fmt::Debug for CachedReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedReader")
            .field("frames", &self.frames)
            .field("position", &self.position)
            .finish()
    }
}

impl CachedReader {
    /// Open a disk entry.
    pub fn open_disk(path: &Path) -> Result<Self, MediaError> {
        let mut file = File::open(path)?;
        let header = read_header(&mut file)?;
        let info = MediaInfo {
            format: header.format,
            sample_rate: header.sample_rate,
            channels: header.channels,
            frames: Some(header.frames),
            gapless: None,
        };
        Ok(Self {
            backing: Backing::Disk(file),
            format: header.sample_format,
            info,
            frames: header.frames,
            scratch: Vec::new(),
            cursor: HEADER_LEN as u64,
            position: 0,
        })
    }

    /// Wrap an entry held in memory.
    pub fn open_memory(
        data: std::sync::Arc<Vec<u8>>,
        sample_rate: u32,
        channels: u16,
        format: PcmSampleFormat,
    ) -> Self {
        let step = 2 * format.bytes_per_sample();
        let frames = data.len() as u64 / step.max(1);
        let info = MediaInfo {
            format: MediaFormat::Other,
            sample_rate,
            channels,
            frames: Some(frames),
            gapless: None,
        };
        Self {
            backing: Backing::Memory(data),
            format,
            info,
            frames,
            scratch: Vec::new(),
            cursor: 0,
            position: 0,
        }
    }

    /// What the entry describes.
    pub fn info(&self) -> &MediaInfo {
        &self.info
    }

    /// Current frame position.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Frames the entry holds, as declared by its header.
    pub fn frames_expected(&self) -> Option<u64> {
        Some(self.frames)
    }

    /// Read up to `out.len() / 2` stereo frames. Returns frames written.
    pub fn read_frames(&mut self, out: &mut [f32]) -> Result<usize, MediaError> {
        let want = out.len() / 2;
        let remaining = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(0);
        let take = want.min(remaining);
        if take == 0 {
            return Ok(0);
        }

        let step = self.format.bytes_per_sample() as usize;
        let needed = take * 2 * step;
        self.scratch.resize(needed, 0);

        let mut filled = 0usize;
        while filled < needed {
            let read = match &mut self.backing {
                Backing::Disk(file) => file.read(&mut self.scratch[filled..needed])?,
                Backing::Memory(data) => {
                    let start = usize::try_from(self.cursor).unwrap_or(usize::MAX) + filled;
                    let available = data.len().saturating_sub(start);
                    let count = available.min(needed - filled);
                    if count > 0 {
                        self.scratch[filled..filled + count]
                            .copy_from_slice(&data[start..start + count]);
                    }
                    count
                }
            };
            if read == 0 {
                break;
            }
            filled += read;
        }

        let produced = filled / (2 * step);
        for (index, slot) in out.iter_mut().enumerate().take(produced * 2) {
            let start = index * step;
            *slot = self.format.decode(&self.scratch[start..start + step]);
        }
        self.position += produced as u64;
        self.cursor += (produced * 2 * step) as u64;
        Ok(produced)
    }

    /// Move to `frame`.
    pub fn seek(&mut self, frame: u64) -> Result<u64, MediaError> {
        let clamped = frame.min(self.frames.saturating_sub(1));
        let step = 2 * self.format.bytes_per_sample();
        let offset = clamped * step;
        self.cursor = match &mut self.backing {
            Backing::Disk(file) => {
                let absolute = HEADER_LEN as u64 + offset;
                file.seek(SeekFrom::Start(absolute))?;
                absolute
            }
            Backing::Memory(_) => offset,
        };
        self.position = clamped;
        Ok(clamped)
    }
}

#[derive(Debug, Clone, Copy)]
struct EntryHeader {
    format: MediaFormat,
    sample_rate: u32,
    channels: u16,
    sample_format: PcmSampleFormat,
    frames: u64,
    data_bytes: u64,
    written_ms: u64,
}

const HEADER_LEN: usize = 48;

fn write_header<W: Write>(
    writer: &mut W,
    info: &MediaInfo,
    sample_format: PcmSampleFormat,
    frames: u64,
) -> std::io::Result<()> {
    writer.write_all(MAGIC)?;
    writer.write_all(&CACHE_FORMAT_VERSION.to_le_bytes())?;
    writer.write_all(&info.sample_rate.to_le_bytes())?;
    writer.write_all(&info.channels.to_le_bytes())?;
    writer.write_all(&sample_format.code().to_le_bytes())?;
    writer.write_all(&[0u8; 6])?;
    writer.write_all(&frames.to_le_bytes())?;
    writer.write_all(&now_ms().to_le_bytes())?;
    writer.write_all(&format_code(info.format).to_le_bytes())?;
    writer.write_all(&[0u8; 2])?;
    Ok(())
}

fn read_header<R: Read>(reader: &mut R) -> Result<EntryHeader, MediaError> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(MediaError::Decode("not a PCM cache entry".to_string()));
    }
    let mut word = [0u8; 4];
    reader.read_exact(&mut word)?;
    if u32::from_le_bytes(word) != CACHE_FORMAT_VERSION {
        return Err(MediaError::Decode(
            "cache entry version mismatch".to_string(),
        ));
    }
    reader.read_exact(&mut word)?;
    let sample_rate = u32::from_le_bytes(word);
    let mut half = [0u8; 2];
    reader.read_exact(&mut half)?;
    let channels = u16::from_le_bytes(half);
    reader.read_exact(&mut half)?;
    let sample_format = PcmSampleFormat::from_code(u16::from_le_bytes(half))
        .ok_or_else(|| MediaError::Decode("unknown sample format in cache entry".to_string()))?;
    let mut skip = [0u8; 6];
    reader.read_exact(&mut skip)?;
    let mut wide = [0u8; 8];
    reader.read_exact(&mut wide)?;
    let frames = u64::from_le_bytes(wide);
    reader.read_exact(&mut wide)?;
    let written_ms = u64::from_le_bytes(wide);
    reader.read_exact(&mut word)?;
    let format = format_from_code(u32::from_le_bytes(word));
    let mut tail = [0u8; 2];
    reader.read_exact(&mut tail)?;

    Ok(EntryHeader {
        format,
        sample_rate,
        channels,
        sample_format,
        frames,
        data_bytes: frames * u64::from(channels) * sample_format.bytes_per_sample(),
        written_ms,
    })
}

fn format_code(format: MediaFormat) -> u32 {
    match format {
        MediaFormat::Wav => 1,
        MediaFormat::Aiff => 2,
        MediaFormat::Flac => 3,
        MediaFormat::Mp3 => 4,
        MediaFormat::OggVorbis => 5,
        MediaFormat::Other => 6,
    }
}

fn format_from_code(code: u32) -> MediaFormat {
    match code {
        1 => MediaFormat::Wav,
        2 => MediaFormat::Aiff,
        3 => MediaFormat::Flac,
        4 => MediaFormat::Mp3,
        5 => MediaFormat::OggVorbis,
        _ => MediaFormat::Other,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Minimal SHA-256, so the key does not pull a dependency just for hashing a file.
///
/// Not constant-time and not used for anything security-relevant: it identifies file content.
#[derive(Debug, Clone)]
struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffered: usize,
    length_bits: u64,
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0u8; 64],
            buffered: 0,
            length_bits: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.length_bits = self.length_bits.wrapping_add((data.len() as u64) * 8);
        while !data.is_empty() {
            let free = 64 - self.buffered;
            let take = free.min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let bits = self.length_bits;
        self.update(&[0x80]);
        while self.buffered != 56 {
            self.update(&[0x00]);
        }
        let block = bits.to_be_bytes();
        self.buffer[56..64].copy_from_slice(&block);
        let padded = self.buffer;
        self.compress(&padded);

        let mut out = [0u8; 32];
        for (index, word) in self.state.iter().enumerate() {
            out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut schedule = [0u32; 64];
        for index in 0..16 {
            schedule[index] = u32::from_be_bytes([
                block[index * 4],
                block[index * 4 + 1],
                block[index * 4 + 2],
                block[index * 4 + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let mut work = self.state;
        for index in 0..64 {
            let s1 = work[4].rotate_right(6) ^ work[4].rotate_right(11) ^ work[4].rotate_right(25);
            let choice = (work[4] & work[5]) ^ ((!work[4]) & work[6]);
            let temp1 = work[7]
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let s0 = work[0].rotate_right(2) ^ work[0].rotate_right(13) ^ work[0].rotate_right(22);
            let majority = (work[0] & work[1]) ^ (work[0] & work[2]) ^ (work[1] & work[2]);
            let temp2 = s0.wrapping_add(majority);

            work[7] = work[6];
            work[6] = work[5];
            work[5] = work[4];
            work[4] = work[3].wrapping_add(temp1);
            work[3] = work[2];
            work[2] = work[1];
            work[1] = work[0];
            work[0] = temp1.wrapping_add(temp2);
        }

        for (slot, extra) in self.state.iter_mut().zip(work) {
            *slot = slot.wrapping_add(extra);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_internal_hash_is_sha256() {
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        assert_eq!(
            hex(&hasher.finish()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut empty = Sha256::new();
        empty.update(b"");
        assert_eq!(
            hex(&empty.finish()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sample_formats_round_trip_within_their_precision() {
        for format in [
            PcmSampleFormat::I16,
            PcmSampleFormat::I24,
            PcmSampleFormat::F32,
        ] {
            let mut bytes = Vec::new();
            for sample in [-1.0f32, -0.5, 0.0, 0.25, 1.0] {
                format.encode(sample, &mut bytes);
            }
            let step = format.bytes_per_sample() as usize;
            let decoded: Vec<f32> = bytes
                .chunks_exact(step)
                .map(|chunk| format.decode(chunk))
                .collect();
            let tolerance = match format {
                PcmSampleFormat::I16 => 1.0 / 32_000.0,
                PcmSampleFormat::I24 => 1.0 / 8_000_000.0,
                PcmSampleFormat::F32 => 0.0,
            };
            for (original, value) in [-1.0f32, -0.5, 0.0, 0.25, 1.0].iter().zip(&decoded) {
                assert!(
                    (original - value).abs() <= tolerance,
                    "{format:?}: {original} -> {value}"
                );
            }
        }
    }

    #[test]
    fn the_policy_reads_the_format_only() {
        let config = CacheConfig::default();
        let lossy = MediaInfo {
            format: MediaFormat::Mp3,
            sample_rate: 44_100,
            channels: 2,
            frames: Some(1),
            gapless: None,
        };
        let lossless = MediaInfo {
            format: MediaFormat::Flac,
            ..lossy.clone()
        };
        assert!(matches!(
            playback_source(&lossy, &config),
            PlaybackSource::PcmCache { .. }
        ));
        assert!(matches!(
            playback_source(&lossless, &config),
            PlaybackSource::Streaming { .. }
        ));
        let disabled = CacheConfig {
            enabled: false,
            ..config
        };
        assert!(matches!(
            playback_source(&lossy, &disabled),
            PlaybackSource::Streaming { .. }
        ));
    }
}
