use std::fs::OpenOptions;
use std::path::PathBuf;

use directories::ProjectDirs;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    EnvFilter, filter::Targets, fmt, prelude::__tracing_subscriber_SubscriberExt,
    util::SubscriberInitExt,
};

const MAX_LOG_SIZE: u64 = 100 * 1024;

fn get_log_path() -> PathBuf {
    let proj_dirs =
        ProjectDirs::from("in", "codito", "xmf").expect("Could not determine project directories");
    let log_dir = proj_dirs.data_dir();
    log_dir.join("xmf.log")
}

fn get_log_dir() -> PathBuf {
    let proj_dirs =
        ProjectDirs::from("in", "codito", "xmf").expect("Could not determine project directories");
    proj_dirs.data_dir().to_path_buf()
}

fn rotate_log_if_needed(log_path: &PathBuf) {
    if let Ok(metadata) = std::fs::metadata(log_path)
        && metadata.len() > MAX_LOG_SIZE
    {
        let old_log = log_path.with_extension("log.old");
        if old_log.exists() {
            let _ = std::fs::remove_file(&old_log);
        }
        let _ = std::fs::rename(log_path, &old_log);
    }
}

pub fn init_logging(verbose: bool) {
    let (level_filter, level) = if verbose {
        (LevelFilter::DEBUG, "debug")
    } else {
        (LevelFilter::OFF, "off")
    };
    let app_filter = Targets::new().with_target("xmf", level_filter);
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    let subscriber = tracing_subscriber::registry()
        .with(app_filter)
        .with(env_filter);

    let has_env_logging = std::env::var("RUST_LOG").is_ok();

    if verbose || has_env_logging {
        let log_dir = get_log_dir();
        std::fs::create_dir_all(&log_dir).expect("Could not create log directory");
        let log_path = get_log_path();

        rotate_log_if_needed(&log_path);

        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .expect("Could not create log file");

        let file_layer = fmt::layer()
            .with_writer(log_file)
            .without_time()
            .with_ansi(false);

        if has_env_logging {
            let stdout_layer = fmt::layer().with_writer(std::io::stderr).without_time();

            let _ = subscriber.with(file_layer).with(stdout_layer).try_init();
        } else {
            let _ = subscriber.with(file_layer).try_init();
        }
    } else {
        let _ = subscriber.try_init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::TempDir;

    #[test]
    fn test_init_logging_non_verbose() {
        init_logging(false);
    }

    #[test]
    fn test_init_logging_verbose() {
        init_logging(true);
    }

    #[test]
    fn test_rotate_log_if_needed_no_rotation() {
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("xmf.log");

        std::fs::write(&log_path, "small content").unwrap();
        rotate_log_if_needed(&log_path);

        assert!(log_path.exists());
        assert!(!log_path.with_extension("log.old").exists());
    }

    #[test]
    fn test_rotate_log_if_needed_rotation() {
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("xmf.log");
        let old_log = log_path.with_extension("log.old");

        std::fs::write(&log_path, "x".repeat(200 * 1024)).unwrap();
        rotate_log_if_needed(&log_path);

        assert!(!log_path.exists());
        assert!(old_log.exists());
    }

    #[test]
    fn test_rotate_log_removes_old() {
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("xmf.log");
        let old_log = log_path.with_extension("log.old");

        std::fs::write(&old_log, "old log").unwrap();
        std::fs::write(&log_path, "x".repeat(200 * 1024)).unwrap();
        rotate_log_if_needed(&log_path);

        assert!(!log_path.exists());
        assert!(old_log.exists());

        let mut contents = String::new();
        std::fs::File::open(&old_log)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert!(contents.starts_with("x"));
    }
}
