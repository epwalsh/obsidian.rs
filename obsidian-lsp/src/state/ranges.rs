use super::*;

pub(in crate::state) fn position_in_location(position: Position, location: &InlineLocation) -> bool {
    let line = position.line + 1;
    let character = position.character;

    line == location.line as u32 && character >= location.col_start as u32 && character < location.col_end as u32
}

pub(in crate::state) fn position_in_range(position: Position, range: &Range) -> bool {
    (position.line > range.start.line
        || (position.line == range.start.line && position.character >= range.start.character))
        && (position.line < range.end.line
            || (position.line == range.end.line && position.character < range.end.character))
}

pub(in crate::state) fn position_in_or_at_range(position: Position, range: &Range) -> bool {
    if range.start == range.end {
        position == range.start
    } else {
        position_in_range(position, range)
    }
}

pub(in crate::state) fn ranges_intersect(left: Range, right: Range) -> bool {
    if left.start == left.end {
        return position_in_or_at_range(left.start, &right);
    }
    if right.start == right.end {
        return position_in_or_at_range(right.start, &left);
    }

    position_before(left.start, right.end) && position_before(right.start, left.end)
}

pub(in crate::state) fn position_before(left: Position, right: Position) -> bool {
    left.line < right.line || (left.line == right.line && left.character < right.character)
}

pub(in crate::state) fn location_to_range(location: &InlineLocation) -> Range {
    Range::new(
        Position::new((location.line.saturating_sub(1)) as u32, location.col_start as u32),
        Position::new((location.line.saturating_sub(1)) as u32, location.col_end as u32),
    )
}

pub(in crate::state) fn line_start_range(line: usize) -> Range {
    Range::new(
        Position::new((line.saturating_sub(1)) as u32, 0),
        Position::new((line.saturating_sub(1)) as u32, 0),
    )
}

pub(in crate::state) fn document_start_range() -> Range {
    Range::new(Position::new(0, 0), Position::new(0, 0))
}

pub(in crate::state) fn document_end_range(text: &str) -> Range {
    let mut line = 0;
    let mut character = 0;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }
    let position = Position::new(line, character);
    Range::new(position, position)
}

pub(in crate::state) fn range_for_span(line: usize, col_start: usize, width: usize) -> Range {
    Range::new(
        Position::new(line as u32, col_start as u32),
        Position::new(line as u32, (col_start + width) as u32),
    )
}
