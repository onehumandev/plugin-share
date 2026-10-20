use crate::models::strip_base64_whitespace;
use crate::state::PluginTempFileManager;
use crate::{Error, SharedFile, MAX_FILE_BYTES};
use base64::{engine::general_purpose, Engine as _};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Returns the directory that holds this plugin's temporary share files.
pub fn plugin_temp_dir() -> Result<PathBuf, Error> {
    let dir = std::env::temp_dir().join("tauri-plugin-share");
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::TempFile(format!("Failed to create temp dir: {}", e)))?;
    Ok(dir)
}

/// Writes every Base64 file to disk and tracks it for cleanup.
pub fn stage_files(
    files: Option<&[SharedFile]>,
    state: &PluginTempFileManager,
) -> Result<Vec<PathBuf>, Error> {
    let Some(files) = files.filter(|files| !files.is_empty()) else {
        return Ok(Vec::new());
    };
    let temp_dir = plugin_temp_dir()?;
    files
        .iter()
        .map(|file| {
            let path = write_shared_file(file, &temp_dir)?;
            state.track(path.clone());
            Ok(path)
        })
        .collect()
}

/// Decodes a Base64 file into `<temp_dir>/<uuid>/<name>`.
///
/// Each file gets its own directory so the original file name is preserved
/// without colliding with other shares.
pub fn write_shared_file(file: &SharedFile, temp_dir: &Path) -> Result<PathBuf, Error> {
    let decoded_bytes = general_purpose::STANDARD
        .decode(strip_base64_whitespace(&file.data))
        .map_err(|_| Error::InvalidArgs("Invalid Base64 data.".to_string()))?;
    if decoded_bytes.len() > MAX_FILE_BYTES {
        return Err(Error::InvalidArgs(format!(
            "File '{}' exceeds the maximum size of {} bytes.",
            file.name, MAX_FILE_BYTES
        )));
    }

    // Security: only keep the final path component so a name like `../x` cannot
    // escape the temporary directory.
    let sanitized_name = Path::new(&file.name)
        .file_name()
        .ok_or_else(|| Error::InvalidArgs("Invalid file name.".to_string()))?
        .to_str()
        .ok_or_else(|| Error::InvalidArgs("File name contains invalid UTF-8.".to_string()))?;

    let file_dir = temp_dir.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&file_dir)
        .map_err(|e| Error::TempFile(format!("Failed to create temp dir: {}", e)))?;
    let temp_path = file_dir.join(sanitized_name);

    let write_result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .and_then(|mut handle| handle.write_all(&decoded_bytes));
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&temp_path);
        let _ = std::fs::remove_dir(&file_dir);
        return Err(Error::TempFile(format!("Failed to write temp file: {}", e)));
    }

    Ok(temp_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn shared_file(data: &str, name: &str) -> SharedFile {
        SharedFile {
            data: data.to_string(),
            name: name.to_string(),
            mime_type: "text/plain".to_string(),
        }
    }

    #[test]
    fn writes_file_with_original_name_in_its_own_directory() {
        let temp_dir = TempDir::new().unwrap();
        let encoded = general_purpose::STANDARD.encode(b"hello world");

        let path = write_shared_file(&shared_file(&encoded, "hello.txt"), temp_dir.path()).unwrap();

        assert_eq!(path.file_name().unwrap(), "hello.txt");
        assert_eq!(path.parent().unwrap().parent().unwrap(), temp_dir.path());
        assert_eq!(fs::read(&path).unwrap(), b"hello world");
    }

    #[test]
    fn accepts_base64_with_line_breaks() {
        let temp_dir = TempDir::new().unwrap();

        let path =
            write_shared_file(&shared_file("aGVs\r\nbG8=\n", "a.txt"), temp_dir.path()).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn rejects_invalid_base64() {
        let temp_dir = TempDir::new().unwrap();

        let err = write_shared_file(
            &shared_file("not-valid-base64!!!", "bad.txt"),
            temp_dir.path(),
        )
        .unwrap_err();

        assert!(
            matches!(err, Error::InvalidArgs(_)),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn sanitizes_path_traversal_in_name() {
        let temp_dir = TempDir::new().unwrap();
        let encoded = general_purpose::STANDARD.encode(b"payload");

        let path = write_shared_file(&shared_file(&encoded, "../etc/secret.txt"), temp_dir.path())
            .unwrap();

        assert_eq!(path.file_name().unwrap(), "secret.txt");
        assert_eq!(path.parent().unwrap().parent().unwrap(), temp_dir.path());
    }
}
