use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Location {
    Frontmatter,
    Inline(InlineLocation),
}

/// Position of an inline element within a text.
///
/// Lines are 1-indexed; columns are 0-indexed and character-based (not byte-based).
/// `col_end` is exclusive (past-the-end).
#[derive(Clone, Debug, PartialEq)]
pub struct InlineLocation {
    pub line: usize,
    pub col_start: usize,
    pub col_end: usize,
}

/// Normalizes a path by resolving `.` and `..` components and making absolute, potentially
/// resolving symlinks.
pub(crate) fn normalize_path(path: &Path, root: &Path) -> PathBuf {
    let path = if path.is_absolute() { path } else { &root.join(path) };
    let mut components: Vec<std::path::Component> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(components.last(), Some(std::path::Component::Normal(_))) {
                    components.pop();
                }
            }
            c => components.push(c),
        }
    }
    components.iter().collect()
}

/// Computes a relative path from `from_dir` to `to_file`.
/// Both arguments must be absolute paths.
pub(crate) fn relative_path(from_dir: &Path, to_file: &Path) -> PathBuf {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = to_file.components().collect();
    let common = from.iter().zip(to.iter()).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in 0..(from.len() - common) {
        result.push("..");
    }
    for c in &to[common..] {
        result.push(c);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::current_dir;

    #[test]
    fn normalize_path_removes_dot() {
        assert_eq!(
            normalize_path(&PathBuf::from("/a/./b"), &current_dir().unwrap()),
            PathBuf::from("/a/b")
        );
    }

    #[test]
    fn normalize_path_resolves_double_dot() {
        assert_eq!(
            normalize_path(&PathBuf::from("/a/b/../c"), &current_dir().unwrap()),
            PathBuf::from("/a/c")
        );
    }

    #[test]
    fn normalize_path_deep_traversal() {
        assert_eq!(
            normalize_path(&PathBuf::from("/a/b/c/../../d"), &current_dir().unwrap(),),
            PathBuf::from("/a/d")
        );
    }

    #[test]
    fn normalize_path_traversal_beyond_root_stops_at_root() {
        // /a/../../b: after processing, ends up as /b (the extra .. can't go above /)
        assert_eq!(
            normalize_path(&PathBuf::from("/a/../../b"), &current_dir().unwrap()),
            PathBuf::from("/b")
        );
    }

    #[test]
    fn normalize_path_starting_with_single_dot() {
        let cwd = current_dir().unwrap();
        assert_eq!(normalize_path(&PathBuf::from("./b"), &cwd), cwd.join("b"));
    }

    #[test]
    fn normalize_path_starting_with_double_dot() {
        let cwd = current_dir().unwrap();
        assert_eq!(
            normalize_path(&PathBuf::from("../b"), &cwd),
            cwd.parent().unwrap().join("b")
        );
    }
}
