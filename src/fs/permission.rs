use std::fs;
use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Ensure file executable permission.
pub fn ensure_executable<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    #[cfg(unix)]
    {
        let metadata = fs::metadata(path)?;
        let mut permissions = metadata.permissions();

        let mode = permissions.mode();

        if mode & 0o111 == 0 {
            permissions.set_mode(mode | 0o111);
            fs::set_permissions(path, permissions)?;
        }
    }

    #[cfg(windows)]
    {
        // Windows does not have executable permission bits.
        fs::metadata(path)?;
    }

    Ok(())
}
