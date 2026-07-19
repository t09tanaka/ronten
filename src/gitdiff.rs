use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Binary,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub section: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    Context,
    Add,
    Remove,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

/// Parse the `a/<old> b/<new>` remainder of a `diff --git ` line, used as
/// the provisional path source (overridden later by `--- `/`+++ ` or
/// `rename from`/`rename to` lines). Returns `None` for either side if the
/// expected `a/`/`b/` structure isn't found (e.g. unusual paths).
fn parse_diff_git_paths(rest: &str) -> (Option<String>, Option<String>) {
    if let Some(old) = rest.strip_prefix("a/") {
        if let Some(idx) = old.find(" b/") {
            let old_path = old[..idx].to_string();
            let new_path = old[idx + 3..].to_string();
            return (Some(old_path), Some(new_path));
        }
    }
    (None, None)
}

/// Strip a leading `a/` or `b/` prefix from a diff path, as used in
/// `--- a/foo` / `+++ b/foo` lines. `/dev/null` is not touched here;
/// callers check for it separately.
fn strip_ab_prefix(path: &str) -> String {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_string()
}

/// Parse a path from a `--- ` / `+++ ` header line's remainder.
/// Returns `None` for `/dev/null`, otherwise the path with `a/`/`b/` stripped.
fn parse_diff_path(rest: &str) -> Option<String> {
    // Git may append a tab and extra info (e.g. timestamps) after the path;
    // only the part before a tab is the path itself.
    let path_part = rest.split('\t').next().unwrap_or(rest).trim_end();
    if path_part == "/dev/null" {
        None
    } else {
        Some(strip_ab_prefix(path_part))
    }
}

/// Parse a unified hunk header of the form:
/// `@@ -{old_start}[,{old_count}] +{new_start}[,{new_count}] @@[ section]`
/// Returns (old_start, old_count, new_start, new_count, section).
fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32, String)> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, section) = match rest.split_once(" @@") {
        Some((r, s)) => (r, s.trim_start().to_string()),
        None => return None,
    };
    let mut parts = ranges.split_whitespace();
    let old_range = parts.next()?;
    let new_range = parts.next()?;

    let (old_start, old_count) = parse_range(old_range.strip_prefix('-')?)?;
    let (new_start, new_count) = parse_range(new_range.strip_prefix('+')?)?;

    Some((old_start, old_count, new_start, new_count, section))
}

fn parse_range(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

pub fn parse_unified_diff(input: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;

    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let (old_path, new_path) = parse_diff_git_paths(rest);
            files.push(FileDiff {
                old_path,
                new_path,
                status: FileStatus::Modified,
                hunks: Vec::new(),
            });
            continue;
        }

        let Some(file) = files.last_mut() else {
            // Content before any `diff --git` line; nothing to attach it to.
            continue;
        };

        // `--- `/`+++ ` file headers only appear before the first hunk of a
        // file; once hunks have started, lines with these prefixes are hunk
        // content (e.g. a removed line whose content starts with "-- ").
        if file.hunks.is_empty() {
            if let Some(rest) = line.strip_prefix("--- ") {
                file.old_path = parse_diff_path(rest);
                continue;
            }
            if let Some(rest) = line.strip_prefix("+++ ") {
                file.new_path = parse_diff_path(rest);
                continue;
            }
        }
        if line.starts_with("new file mode") {
            file.status = FileStatus::Added;
            continue;
        }
        if line.starts_with("deleted file mode") {
            file.status = FileStatus::Deleted;
            continue;
        }
        if let Some(rest) = line.strip_prefix("rename from ") {
            file.status = FileStatus::Renamed;
            file.old_path = Some(rest.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("rename to ") {
            file.status = FileStatus::Renamed;
            file.new_path = Some(rest.to_string());
            continue;
        }
        if (line.starts_with("Binary files") && line.ends_with("differ"))
            || line.starts_with("GIT binary patch")
        {
            file.status = FileStatus::Binary;
            continue;
        }

        if let Some((old_start, old_count, new_start, new_count, section)) = parse_hunk_header(line)
        {
            old_no = old_start;
            new_no = new_start;
            file.hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                section,
                lines: Vec::new(),
            });
            continue;
        }

        if line.starts_with('\\') {
            // "\ No newline at end of file" marker; not a content line.
            continue;
        }

        let Some(hunk) = file.hunks.last_mut() else {
            continue;
        };

        let mut chars = line.chars();
        let marker = chars.next();
        let content = chars.as_str().to_string();

        match marker {
            // A genuinely blank line in a hunk still counts as context;
            // `chars.next()` returns `None` for it rather than `Some(' ')`.
            Some(' ') | None => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Context,
                    content,
                    old_no: Some(old_no),
                    new_no: Some(new_no),
                });
                old_no += 1;
                new_no += 1;
            }
            Some('-') => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Remove,
                    content,
                    old_no: Some(old_no),
                    new_no: None,
                });
                old_no += 1;
            }
            Some('+') => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Add,
                    content,
                    old_no: None,
                    new_no: Some(new_no),
                });
                new_no += 1;
            }
            Some(_) => {
                // Unrecognized marker character; not a valid diff content
                // line, so skip it rather than misclassifying it.
            }
        }
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFIED: &str = "\
diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,4 +1,5 @@ header-one
 line1
-old2
+new2
+new3
 line4
@@ -10,3 +11,3 @@ header-two
 a
-b
+B
 c
";

    #[test]
    fn parses_modified_file_with_two_hunks() {
        let files = parse_unified_diff(MODIFIED);
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.new_path.as_deref(), Some("src/app.ts"));
        assert_eq!(f.status, FileStatus::Modified);
        assert_eq!(f.hunks.len(), 2);
        let h = &f.hunks[0];
        assert_eq!(
            (h.old_start, h.old_count, h.new_start, h.new_count),
            (1, 4, 1, 5)
        );
        assert_eq!(h.section, "header-one");
        // line numbering
        assert_eq!(h.lines[0].old_no, Some(1));
        assert_eq!(h.lines[0].new_no, Some(1));
        assert_eq!(h.lines[1].kind as u8, LineKind::Remove as u8);
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(h.lines[1].new_no, None);
        assert_eq!(h.lines[2].new_no, Some(2));
        assert_eq!(h.lines[3].new_no, Some(3));
        assert_eq!(h.lines[4].old_no, Some(3));
        assert_eq!(h.lines[4].new_no, Some(4));
    }

    const ADDED: &str = "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
";

    #[test]
    fn parses_added_file() {
        let f = &parse_unified_diff(ADDED)[0];
        assert_eq!(f.status, FileStatus::Added);
        assert_eq!(f.old_path, None);
        assert_eq!(f.new_path.as_deref(), Some("new.txt"));
        assert_eq!(f.hunks[0].new_count, 2);
    }

    const DELETED: &str = "\
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 4444444..0000000
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-bye
-now
";

    #[test]
    fn parses_deleted_file() {
        let f = &parse_unified_diff(DELETED)[0];
        assert_eq!(f.status, FileStatus::Deleted);
        assert_eq!(f.new_path, None);
        assert_eq!(f.hunks[0].new_count, 0);
        assert_eq!(f.hunks[0].lines[0].new_no, None);
    }

    const RENAME_NO_CHANGE: &str = "\
diff --git a/old_name.rs b/new_name.rs
similarity index 100%
rename from old_name.rs
rename to new_name.rs
";

    #[test]
    fn parses_pure_rename_without_hunks() {
        let f = &parse_unified_diff(RENAME_NO_CHANGE)[0];
        assert_eq!(f.status, FileStatus::Renamed);
        assert_eq!(f.old_path.as_deref(), Some("old_name.rs"));
        assert_eq!(f.new_path.as_deref(), Some("new_name.rs"));
        assert!(f.hunks.is_empty());
    }

    const BINARY: &str = "\
diff --git a/logo.png b/logo.png
index 5555555..6666666 100644
Binary files a/logo.png and b/logo.png differ
";

    #[test]
    fn parses_binary_file() {
        let f = &parse_unified_diff(BINARY)[0];
        assert_eq!(f.status, FileStatus::Binary);
        assert!(f.hunks.is_empty());
        assert_eq!(f.new_path.as_deref(), Some("logo.png"));
    }

    #[test]
    fn skips_no_newline_marker() {
        let d = "\
diff --git a/x b/x
index 1..2 100644
--- a/x
+++ b/x
@@ -1 +1 @@
-a
\\ No newline at end of file
+b
\\ No newline at end of file
";
        let f = &parse_unified_diff(d)[0];
        assert_eq!(f.hunks[0].lines.len(), 2);
    }

    #[test]
    fn multiple_files() {
        let combined = format!("{MODIFIED}{ADDED}");
        assert_eq!(parse_unified_diff(&combined).len(), 2);
    }

    #[test]
    fn header_like_content_lines_are_not_misparsed() {
        // A removed line whose content starts with "-- " renders as "--- ..."
        // and an added line whose content starts with "++ " renders as
        // "+++ ..." — these must be treated as hunk content, not file headers.
        let d = "\
diff --git a/query.sql b/query.sql
index 1111111..2222222 100644
--- a/query.sql
+++ b/query.sql
@@ -1,3 +1,3 @@
 SELECT 1;
--- old comment
+++ new comment
 SELECT 2;
";
        let files = parse_unified_diff(d);
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.old_path.as_deref(), Some("query.sql"));
        assert_eq!(f.new_path.as_deref(), Some("query.sql"));
        let h = &f.hunks[0];
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[0].kind as u8, LineKind::Context as u8);
        assert_eq!(h.lines[1].kind as u8, LineKind::Remove as u8);
        assert_eq!(h.lines[1].content, "-- old comment");
        assert_eq!(h.lines[2].kind as u8, LineKind::Add as u8);
        assert_eq!(h.lines[2].content, "++ new comment");
        assert_eq!(h.lines[3].kind as u8, LineKind::Context as u8);
        assert_eq!(h.lines[3].old_no, Some(3));
        assert_eq!(h.lines[3].new_no, Some(3));
    }
}

#[derive(Debug)]
pub enum GitError {
    NotARepo,
    BadBase(String),
    GitFailed(String),
}

/// Repo root of cwd, or `NotARepo`. Uses `git rev-parse --show-toplevel`.
pub fn repo_root() -> Result<std::path::PathBuf, GitError> {
    let output = std::process::Command::new("git")
        .env("LC_ALL", "C")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|_| GitError::NotARepo)?;
    if !output.status.success() {
        return Err(GitError::NotARepo);
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(std::path::PathBuf::from(path))
}

/// Current branch name (for `--title` default). `git rev-parse --abbrev-ref HEAD`;
/// on any failure returns `"review"`.
pub fn current_branch(root: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .env("LC_ALL", "C")
        .current_dir(root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if branch.is_empty() {
                "review".to_string()
            } else {
                branch
            }
        }
        _ => "review".to_string(),
    }
}

/// Runs `git -C <root> diff <base>...HEAD --no-color --unified=3 --find-renames`
/// and parses the result. Empty stdout on success is an empty vec. Non-zero
/// exit with a stderr indicating an unrecognized revision is classified as
/// `BadBase`; any other non-zero exit is `GitFailed`.
pub fn compute_diff(root: &std::path::Path, base: &str) -> Result<Vec<FileDiff>, GitError> {
    let output = std::process::Command::new("git")
        .env("LC_ALL", "C")
        .arg("-C")
        .arg(root)
        .args([
            "diff",
            &format!("{base}...HEAD"),
            "--no-color",
            "--unified=3",
            "--find-renames",
        ])
        .output()
        .map_err(|e| GitError::GitFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if stderr.contains("unknown revision")
            || stderr.contains("bad revision")
            || stderr.contains("ambiguous argument")
        {
            return Err(GitError::BadBase(stderr));
        }
        return Err(GitError::GitFailed(stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.is_empty() {
        return Ok(Vec::new());
    }
    Ok(parse_unified_diff(&stdout))
}

#[cfg(test)]
mod git_tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let st = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    fn fixture_repo() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "user.name", "t"]);
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base"]);
        git(d, &["checkout", "-b", "feature"]);
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        std::fs::write(d.join("b.txt"), "new file\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);
        td
    }

    #[test]
    fn computes_diff_against_base() {
        let td = fixture_repo();
        let files = compute_diff(td.path(), "main").unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.new_path.as_deref() == Some("a.txt")));
        assert!(files.iter().any(|f| f.status == FileStatus::Added));
    }

    #[test]
    fn bad_base_is_distinguished() {
        let td = fixture_repo();
        assert!(matches!(
            compute_diff(td.path(), "no-such-ref"),
            Err(GitError::BadBase(_))
        ));
    }

    #[test]
    fn empty_diff_returns_empty_vec() {
        let td = fixture_repo();
        let files = compute_diff(td.path(), "HEAD").unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn current_branch_name() {
        let td = fixture_repo();
        assert_eq!(current_branch(td.path()), "feature");
    }
}
