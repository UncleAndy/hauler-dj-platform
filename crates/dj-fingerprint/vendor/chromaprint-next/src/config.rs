use crate::fingerprint::classifier::Classifier;
use crate::fingerprint::filter::HaarFilter;
use crate::fingerprint::quantizer::Quantizer;

/// Default internal sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 11025;

/// Default FFT frame size.
pub const DEFAULT_FRAME_SIZE: usize = 4096;

/// Default FFT frame overlap.
pub const DEFAULT_FRAME_OVERLAP: usize = DEFAULT_FRAME_SIZE - DEFAULT_FRAME_SIZE / 3;

/// Minimum frequency for chroma extraction (Hz).
pub const MIN_FREQ: u32 = 28;

/// Maximum frequency for chroma extraction (Hz).
pub const MAX_FREQ: u32 = 3520;

/// Default chroma filter coefficients (5-tap symmetric FIR).
pub const CHROMA_FILTER_COEFFICIENTS: &[f64] = &[0.25, 0.75, 1.0, 0.75, 0.25];

/// Configuration for a fingerprinting algorithm variant.
#[derive(Debug, Clone)]
pub struct Config {
    pub classifiers: &'static [Classifier],
    pub filter_coefficients: &'static [f64],
    pub interpolate: bool,
    pub remove_silence: bool,
    pub silence_threshold: i16,
    pub frame_size: usize,
    pub frame_overlap: usize,
    pub sample_rate: u32,
}

impl Config {
    #[inline]
    pub fn frame_increment(&self) -> usize {
        self.frame_size - self.frame_overlap
    }

    pub fn max_filter_width(&self) -> usize {
        self.classifiers
            .iter()
            .map(|c| c.filter.width as usize)
            .max()
            .unwrap_or(0)
    }
}

static CLASSIFIERS_TEST1: [Classifier; 16] = [
    Classifier {
        filter: HaarFilter {
            kind: 0,
            y: 0,
            height: 3,
            width: 15,
        },
        quantizer: Quantizer {
            t0: 2.10543,
            t1: 2.45354,
            t2: 2.69414,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 1,
            y: 0,
            height: 4,
            width: 14,
        },
        quantizer: Quantizer {
            t0: -0.345922,
            t1: 0.0463746,
            t2: 0.446251,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 1,
            y: 4,
            height: 4,
            width: 11,
        },
        quantizer: Quantizer {
            t0: -0.392132,
            t1: 0.0291077,
            t2: 0.443391,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 0,
            height: 4,
            width: 14,
        },
        quantizer: Quantizer {
            t0: -0.192851,
            t1: 0.00583535,
            t2: 0.204053,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 8,
            height: 2,
            width: 4,
        },
        quantizer: Quantizer {
            t0: -0.0771619,
            t1: -0.00991999,
            t2: 0.0575406,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 5,
            y: 6,
            height: 2,
            width: 15,
        },
        quantizer: Quantizer {
            t0: -0.710437,
            t1: -0.518954,
            t2: -0.330402,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 1,
            y: 9,
            height: 2,
            width: 16,
        },
        quantizer: Quantizer {
            t0: -0.353724,
            t1: -0.0189719,
            t2: 0.289768,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 4,
            height: 2,
            width: 10,
        },
        quantizer: Quantizer {
            t0: -0.128418,
            t1: -0.0285697,
            t2: 0.0591791,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 9,
            height: 2,
            width: 16,
        },
        quantizer: Quantizer {
            t0: -0.139052,
            t1: -0.0228468,
            t2: 0.0879723,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 1,
            height: 3,
            width: 6,
        },
        quantizer: Quantizer {
            t0: -0.133562,
            t1: 0.00669205,
            t2: 0.155012,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 3,
            height: 6,
            width: 2,
        },
        quantizer: Quantizer {
            t0: -0.0267,
            t1: 0.00804829,
            t2: 0.0459773,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 8,
            height: 1,
            width: 10,
        },
        quantizer: Quantizer {
            t0: -0.0972417,
            t1: 0.0152227,
            t2: 0.129003,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 4,
            height: 4,
            width: 14,
        },
        quantizer: Quantizer {
            t0: -0.141434,
            t1: 0.00374515,
            t2: 0.149935,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 5,
            y: 4,
            height: 2,
            width: 15,
        },
        quantizer: Quantizer {
            t0: -0.64035,
            t1: -0.466999,
            t2: -0.285493,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 5,
            y: 9,
            height: 2,
            width: 3,
        },
        quantizer: Quantizer {
            t0: -0.322792,
            t1: -0.254258,
            t2: -0.174278,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 1,
            height: 8,
            width: 4,
        },
        quantizer: Quantizer {
            t0: -0.0741375,
            t1: -0.00590933,
            t2: 0.0600357,
        },
    },
];

static CLASSIFIERS_TEST2: [Classifier; 16] = [
    Classifier {
        filter: HaarFilter {
            kind: 0,
            y: 4,
            height: 3,
            width: 15,
        },
        quantizer: Quantizer {
            t0: 1.98215,
            t1: 2.35817,
            t2: 2.63523,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 4,
            y: 4,
            height: 6,
            width: 15,
        },
        quantizer: Quantizer {
            t0: -1.03809,
            t1: -0.651211,
            t2: -0.282167,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 1,
            y: 0,
            height: 4,
            width: 16,
        },
        quantizer: Quantizer {
            t0: -0.298702,
            t1: 0.119262,
            t2: 0.558497,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 8,
            height: 2,
            width: 12,
        },
        quantizer: Quantizer {
            t0: -0.105439,
            t1: 0.0153946,
            t2: 0.135898,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 4,
            height: 4,
            width: 8,
        },
        quantizer: Quantizer {
            t0: -0.142891,
            t1: 0.0258736,
            t2: 0.200632,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 4,
            y: 0,
            height: 3,
            width: 5,
        },
        quantizer: Quantizer {
            t0: -0.826319,
            t1: -0.590612,
            t2: -0.368214,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 1,
            y: 2,
            height: 2,
            width: 9,
        },
        quantizer: Quantizer {
            t0: -0.557409,
            t1: -0.233035,
            t2: 0.0534525,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 7,
            height: 3,
            width: 4,
        },
        quantizer: Quantizer {
            t0: -0.0646826,
            t1: 0.00620476,
            t2: 0.0784847,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 6,
            height: 2,
            width: 16,
        },
        quantizer: Quantizer {
            t0: -0.192387,
            t1: -0.029699,
            t2: 0.215855,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 1,
            height: 3,
            width: 2,
        },
        quantizer: Quantizer {
            t0: -0.0397818,
            t1: -0.00568076,
            t2: 0.0292026,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 5,
            y: 10,
            height: 1,
            width: 15,
        },
        quantizer: Quantizer {
            t0: -0.53823,
            t1: -0.369934,
            t2: -0.190235,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 6,
            height: 2,
            width: 10,
        },
        quantizer: Quantizer {
            t0: -0.124877,
            t1: 0.0296483,
            t2: 0.139239,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 1,
            height: 1,
            width: 14,
        },
        quantizer: Quantizer {
            t0: -0.101475,
            t1: 0.0225617,
            t2: 0.231971,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 5,
            height: 6,
            width: 4,
        },
        quantizer: Quantizer {
            t0: -0.0799915,
            t1: -0.00729616,
            t2: 0.063262,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 1,
            y: 9,
            height: 2,
            width: 12,
        },
        quantizer: Quantizer {
            t0: -0.272556,
            t1: 0.019424,
            t2: 0.302559,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 4,
            height: 2,
            width: 14,
        },
        quantizer: Quantizer {
            t0: -0.164292,
            t1: -0.0321188,
            t2: 0.0846339,
        },
    },
];

// TEST3 uses the same classifier values as TEST2.
static CLASSIFIERS_TEST3: [Classifier; 16] = [
    Classifier {
        filter: HaarFilter {
            kind: 0,
            y: 4,
            height: 3,
            width: 15,
        },
        quantizer: Quantizer {
            t0: 1.98215,
            t1: 2.35817,
            t2: 2.63523,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 4,
            y: 4,
            height: 6,
            width: 15,
        },
        quantizer: Quantizer {
            t0: -1.03809,
            t1: -0.651211,
            t2: -0.282167,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 1,
            y: 0,
            height: 4,
            width: 16,
        },
        quantizer: Quantizer {
            t0: -0.298702,
            t1: 0.119262,
            t2: 0.558497,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 8,
            height: 2,
            width: 12,
        },
        quantizer: Quantizer {
            t0: -0.105439,
            t1: 0.0153946,
            t2: 0.135898,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 4,
            height: 4,
            width: 8,
        },
        quantizer: Quantizer {
            t0: -0.142891,
            t1: 0.0258736,
            t2: 0.200632,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 4,
            y: 0,
            height: 3,
            width: 5,
        },
        quantizer: Quantizer {
            t0: -0.826319,
            t1: -0.590612,
            t2: -0.368214,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 1,
            y: 2,
            height: 2,
            width: 9,
        },
        quantizer: Quantizer {
            t0: -0.557409,
            t1: -0.233035,
            t2: 0.0534525,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 7,
            height: 3,
            width: 4,
        },
        quantizer: Quantizer {
            t0: -0.0646826,
            t1: 0.00620476,
            t2: 0.0784847,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 6,
            height: 2,
            width: 16,
        },
        quantizer: Quantizer {
            t0: -0.192387,
            t1: -0.029699,
            t2: 0.215855,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 1,
            height: 3,
            width: 2,
        },
        quantizer: Quantizer {
            t0: -0.0397818,
            t1: -0.00568076,
            t2: 0.0292026,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 5,
            y: 10,
            height: 1,
            width: 15,
        },
        quantizer: Quantizer {
            t0: -0.53823,
            t1: -0.369934,
            t2: -0.190235,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 6,
            height: 2,
            width: 10,
        },
        quantizer: Quantizer {
            t0: -0.124877,
            t1: 0.0296483,
            t2: 0.139239,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 2,
            y: 1,
            height: 1,
            width: 14,
        },
        quantizer: Quantizer {
            t0: -0.101475,
            t1: 0.0225617,
            t2: 0.231971,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 5,
            height: 6,
            width: 4,
        },
        quantizer: Quantizer {
            t0: -0.0799915,
            t1: -0.00729616,
            t2: 0.063262,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 1,
            y: 9,
            height: 2,
            width: 12,
        },
        quantizer: Quantizer {
            t0: -0.272556,
            t1: 0.019424,
            t2: 0.302559,
        },
    },
    Classifier {
        filter: HaarFilter {
            kind: 3,
            y: 4,
            height: 2,
            width: 14,
        },
        quantizer: Quantizer {
            t0: -0.164292,
            t1: -0.0321188,
            t2: 0.0846339,
        },
    },
];

pub static CONFIG_TEST1: Config = Config {
    classifiers: &CLASSIFIERS_TEST1,
    filter_coefficients: CHROMA_FILTER_COEFFICIENTS,
    interpolate: false,
    remove_silence: false,
    silence_threshold: 0,
    frame_size: DEFAULT_FRAME_SIZE,
    frame_overlap: DEFAULT_FRAME_OVERLAP,
    sample_rate: DEFAULT_SAMPLE_RATE,
};

pub static CONFIG_TEST2: Config = Config {
    classifiers: &CLASSIFIERS_TEST2,
    filter_coefficients: CHROMA_FILTER_COEFFICIENTS,
    interpolate: false,
    remove_silence: false,
    silence_threshold: 0,
    frame_size: DEFAULT_FRAME_SIZE,
    frame_overlap: DEFAULT_FRAME_OVERLAP,
    sample_rate: DEFAULT_SAMPLE_RATE,
};

pub static CONFIG_TEST3: Config = Config {
    classifiers: &CLASSIFIERS_TEST3,
    filter_coefficients: CHROMA_FILTER_COEFFICIENTS,
    // NOTE: C sets interpolate(true) in config, but fingerprinter.cpp never calls
    // m_chroma->set_interpolate(true) (it's commented out on line 32), so interpolate
    // is effectively always false. We match that behavior here.
    interpolate: false,
    remove_silence: false,
    silence_threshold: 0,
    frame_size: DEFAULT_FRAME_SIZE,
    // NOTE: C's FingerprinterConfigurationTest3 constructor never calls set_frame_overlap(),
    // so it stays at the base class default of 0. We match that bug here.
    frame_overlap: 0,
    sample_rate: DEFAULT_SAMPLE_RATE,
};

pub static CONFIG_TEST4: Config = Config {
    classifiers: &CLASSIFIERS_TEST2,
    filter_coefficients: CHROMA_FILTER_COEFFICIENTS,
    interpolate: false,
    remove_silence: true,
    silence_threshold: 50,
    frame_size: DEFAULT_FRAME_SIZE,
    frame_overlap: DEFAULT_FRAME_OVERLAP,
    sample_rate: DEFAULT_SAMPLE_RATE,
};

pub static CONFIG_TEST5: Config = Config {
    classifiers: &CLASSIFIERS_TEST2,
    filter_coefficients: CHROMA_FILTER_COEFFICIENTS,
    interpolate: false,
    remove_silence: false,
    silence_threshold: 0,
    frame_size: DEFAULT_FRAME_SIZE / 2,
    frame_overlap: DEFAULT_FRAME_SIZE / 2 - DEFAULT_FRAME_SIZE / 4,
    sample_rate: DEFAULT_SAMPLE_RATE,
};

/// Get the configuration for a given algorithm.
pub fn get_config(algorithm: crate::types::Algorithm) -> &'static Config {
    match algorithm {
        crate::types::Algorithm::Test1 => &CONFIG_TEST1,
        crate::types::Algorithm::Test2 => &CONFIG_TEST2,
        crate::types::Algorithm::Test3 => &CONFIG_TEST3,
        crate::types::Algorithm::Test4 => &CONFIG_TEST4,
        crate::types::Algorithm::Test5 => &CONFIG_TEST5,
    }
}
