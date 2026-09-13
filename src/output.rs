//! Publish completed output with one rename, leaving existing files intact
//! after query, formatting, or write failures.
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct AtomicOutput {
    target: PathBuf,
    temporary: PathBuf,
}

impl AtomicOutput {
    pub fn create(path: &Path) -> Result<(Self, File), String> {
        let permissions = match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => Some(metadata.permissions()),
            Ok(_) => return Err("output destination must be a regular file".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let temporary = parent.join(format!(
                ".phosphor-output-{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temporary) {
                Ok(file) => {
                    let output = Self {
                        target: path.to_owned(),
                        temporary,
                    };
                    if let Some(permissions) = permissions {
                        file.set_permissions(permissions)
                            .map_err(|e| e.to_string())?;
                    }
                    return Ok((output, file));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("could not create temporary output file".into())
    }

    pub fn publish(self) -> Result<(), String> {
        std::fs::rename(&self.temporary, &self.target).map_err(|e| e.to_string())
    }
}

impl Drop for AtomicOutput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temporary);
    }
}
