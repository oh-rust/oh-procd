use chrono::Local;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use crate::config::ProcessConfig;

fn current_hour() -> String {
    Local::now().format("%Y%m%d%H").to_string()
}

pub fn pipe_logger(
    mut reader: impl std::io::Read + Send + 'static,
    cfg: ProcessConfig,
    kind: &'static str,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];

        let mut file: Option<std::fs::File> = None;
        let mut active_hour = current_hour();

        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break, //  EOF
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("read pipe failed: {:?}", e);
                    break;
                }
            };

            let dir = Path::new(&cfg.output_dir);
            if !dir.exists() {
                match fs::create_dir_all(dir) {
                    Ok(()) => {}
                    Err(e) => {
                        tracing::warn!("create log_dir({:?}) {:?}", dir, e.to_string());
                        continue;
                    }
                }
            }

            let hour = current_hour();
            let path = dir.join(format!("{kind}.{hour}.log"));
            let need_rotate = hour != active_hour;
            active_hour = hour;

            let missing = fs::metadata(&path).is_err();

            if missing || need_rotate || file.is_none() {
                match OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(Path::new(&path))
                {
                    Ok(f) => {
                        file = Some(f);
                        tracing::info!("open_log {:?}", &path);
                    }
                    Err(e) => {
                        tracing::warn!("open_log failed {:?}", e);
                    }
                };
            }

            if let Some(f) = file.as_mut() {
                if let Err(e) = f.write_all(&buf[..n]) {
                    tracing::warn!("write log failed: {:?}", e);
                    file = None
                }
            }
        }
    });
}
