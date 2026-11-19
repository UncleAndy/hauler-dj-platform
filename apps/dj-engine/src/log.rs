// The engine's own log: a file next to the collection, so a machine without a console still has evidence.
//
// The DJ runs the engine from a launcher on Windows, and the console it opens is easy to lose — or not to have at
// all. Every line written here goes to stderr too, so a terminal that is there sees the same thing, and to
// `<data>/hauler-dj/logs/dj-engine.log`, so a terminal that is not there answers later.

use std::io::Write;

/// Where the engine writes its log.
pub fn log_path() -> Option<std::path::PathBuf> {
    dj_paths::data_dir().map(|base| base.join("hauler-dj").join("logs").join("dj-engine.log"))
}

/// One line to the log, with the time it happened.
pub fn info(message: impl AsRef<str>) {
    let message = message.as_ref();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0);
    eprintln!("dj-engine: {message}");
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{stamp} dj-engine: {message}");
    }
}

/// The name of a command worth a line in the log, or `None` for the one the deck sends while a hand moves.
///
/// A scratch or a seek arrives every block; a load or a page question arrives when the DJ asks for something. The
/// log is for the second kind, and a log that fills with the first is a log nobody reads.
pub fn command_name(command: &dj_domain::DjCommand) -> Option<&'static str> {
    use dj_domain::DjCommand;
    match command {
        DjCommand::Load { .. } => Some("Load"),
        DjCommand::Eject { .. } => Some("Eject"),
        DjCommand::Import { .. } => Some("Import"),
        DjCommand::Rescan { .. } => Some("Rescan"),
        DjCommand::ImportCancel => Some("ImportCancel"),
        DjCommand::ImportAnswer { .. } => Some("ImportAnswer"),
        DjCommand::AnalyzeTrack { .. } => Some("AnalyzeTrack"),
        DjCommand::AnalyzeTracks { .. } => Some("AnalyzeTracks"),
        DjCommand::AnalyzeCancel => Some("AnalyzeCancel"),
        DjCommand::AnalyzeTempo { .. } => Some("AnalyzeTempo"),
        DjCommand::SetAnalysisLock { .. } => Some("SetAnalysisLock"),
        DjCommand::EditGrid { .. } => Some("EditGrid"),
        DjCommand::Model { .. } => Some("Model"),
        DjCommand::QueryCollection { .. } => Some("QueryCollection"),
        _ => None,
    }
}
