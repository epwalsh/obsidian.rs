use super::*;

pub(in crate::state) fn make_diagnostic(range: Range, code: &str, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String(code.to_string())),
        source: Some("obsidian-lsp".to_string()),
        message,
        ..Default::default()
    }
}

pub(in crate::state) fn diagnostic_code_is(diagnostic: &Diagnostic, expected: &str) -> bool {
    matches!(
        diagnostic.code.as_ref(),
        Some(NumberOrString::String(code)) if code == expected
    )
}

pub(in crate::state) fn diagnostic_backtick_value(diagnostic: &Diagnostic) -> Option<String> {
    let (_, after_open) = diagnostic.message.split_once('`')?;
    let (value, _) = after_open.split_once('`')?;
    Some(value.to_string())
}

pub(in crate::state) fn matching_diagnostics(diagnostics: &[Diagnostic], code: &str, range: Range) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic_code_is(diagnostic, code))
        .filter(|diagnostic| ranges_intersect(diagnostic.range, range))
        .cloned()
        .collect()
}

pub(in crate::state) fn diagnostic_applies_to_request(
    diagnostic: &Diagnostic,
    range: Range,
    position: Position,
) -> bool {
    range_applies_to_request(diagnostic.range, range, position)
}

pub(in crate::state) fn diagnostic_applies_to_request_range(
    diagnostic_range: Range,
    range: Range,
    position: Position,
) -> bool {
    range_applies_to_request(diagnostic_range, range, position)
}

pub(in crate::state) fn range_applies_to_request(target: Range, request: Range, position: Position) -> bool {
    ranges_intersect(target, request)
        || position_in_or_at_range(position, &target)
        || range_lines_intersect(target, request)
        || position_on_range_line(position, &target)
}

pub(in crate::state) fn range_lines_intersect(left: Range, right: Range) -> bool {
    left.start.line <= right.end.line && right.start.line <= left.end.line
}

pub(in crate::state) fn position_on_range_line(position: Position, range: &Range) -> bool {
    position.line >= range.start.line && position.line <= range.end.line
}

pub(in crate::state) fn build_ignore_set(patterns: &[String]) -> globset::GlobSet {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        if let Ok(glob) = globset::GlobBuilder::new(pattern).case_insensitive(false).build() {
            builder.add(glob);
        }
    }
    builder.build().unwrap_or_default()
}
