/// `read_file` renders `{line:>6}  {text}`. Copied spans come back in two
/// shapes: the whole gutter (`"   155  "` plus the text) or, much more often,
/// only the two spaces after the line number. Either way each source line is
/// present and the joined span matches nothing. Accept a stripped span only
/// when it occurs once. An exact match wins first; a single extra-indented
/// line, or a strip that hits more than one place, is left untouched.
pub(super) fn match_edit_text(content: &str, requested: &str) -> Option<String> {
    if content.contains(requested) {
        return Some(requested.to_owned());
    }
    let lines: Vec<&str> = requested.lines().collect();
    strip_numbered_gutter(content, &lines).or_else(|| strip_copied_gutter_gap(content, &lines))
}

fn strip_numbered_gutter(content: &str, lines: &[&str]) -> Option<String> {
    let mut stripped = Vec::with_capacity(lines.len());
    let mut saw_text = false;
    for line in lines {
        if line.trim().is_empty() {
            stripped.push(String::new());
            continue;
        }
        let width = read_gutter_width(line)?;
        saw_text = true;
        stripped.push(line[width..].to_owned());
    }
    if !saw_text {
        return None;
    }
    unique_span(content, &stripped.join("\n"))
}

/// The line number was dropped and the two separator spaces stayed, in front
/// of the real indentation. One line is ambiguous with ordinary indentation,
/// so this only applies to a multi-line span.
fn strip_copied_gutter_gap(content: &str, lines: &[&str]) -> Option<String> {
    let mut stripped = Vec::with_capacity(lines.len());
    let mut substantive = 0;
    for line in lines {
        if line.trim().is_empty() {
            stripped.push(String::new());
            continue;
        }
        let rest = line.strip_prefix("  ")?;
        substantive += 1;
        stripped.push(rest.to_owned());
    }
    if substantive < 2 {
        return None;
    }
    unique_span(content, &stripped.join("\n"))
}

fn unique_span(content: &str, stripped: &str) -> Option<String> {
    if stripped.is_empty() || content.matches(stripped).count() != 1 {
        None
    } else {
        Some(stripped.to_owned())
    }
}

/// Width of a `read_file` gutter, `{:>6}  {text}`.
///
/// The line number is right-aligned in six columns, so the gutter starts with
/// spaces for every line below 100000. A wider number is emitted in full and
/// still ends with the two separator spaces.
fn read_gutter_width(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    if bytes.len() < 8 || bytes[6..8] != *b"  " {
        return None;
    }
    let field = &bytes[..6];
    let digits_start = field.iter().position(|byte| *byte != b' ')?;
    if field[digits_start..].iter().all(u8::is_ascii_digit) {
        Some(8)
    } else {
        None
    }
}
