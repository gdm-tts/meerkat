//! The repository list: a plain text file with one local path per line.
//! Empty lines and lines starting with `#` are ignored.

use std::path::{Path, PathBuf};

pub const DEFAULT_FILE: &str = "meerkat.txt";

pub fn load(file: &Path) -> std::io::Result<Vec<PathBuf>> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(parse(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

pub fn save(file: &Path, repos: &[PathBuf]) -> std::io::Result<()> {
    let mut text = String::new();
    for repo in repos {
        text.push_str(&repo.to_string_lossy());
        text.push('\n');
    }
    std::fs::write(file, text)
}

fn parse(text: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for line in text.lines() {
        // Tolerate a UTF-8 BOM written by some Windows editors.
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = PathBuf::from(line);
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_comments_blanks_and_duplicates() {
        let text = "\u{feff}/a/b\n\n# comment\n  /c d/e  \r\n/a/b\n";
        assert_eq!(
            parse(text),
            vec![PathBuf::from("/a/b"), PathBuf::from("/c d/e")]
        );
    }
}
