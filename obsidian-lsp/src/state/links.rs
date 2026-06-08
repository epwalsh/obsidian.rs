use super::*;

pub(in crate::state) fn markdown_link_text(source_path: &Path, link: &Link, target: &Note) -> String {
    let Link::Wiki { heading, alias, .. } = link else {
        unreachable!("markdown_link_text should only be called for wiki links");
    };
    let display_text = alias
        .as_deref()
        .or(target.title.as_deref())
        .unwrap_or(target.id.as_str());
    let mut url = markdown_url_for_target(source_path, &target.path);
    if let Some(heading) = heading.as_deref().filter(|heading| !heading.is_empty()) {
        url.push('#');
        url.push_str(&percent_encode_url_component(heading));
    }
    format!("[{display_text}]({url})")
}

pub(in crate::state) fn wiki_link_text(
    snapshot: &StateSnapshot,
    link: &Link,
    target: &Note,
) -> Result<String, StateError> {
    let Link::Markdown { text, .. } = link else {
        unreachable!("wiki_link_text should only be called for markdown links");
    };
    let target_name = note_file_stem(target);
    let mut wiki = format!("[[{target_name}");
    if let Some(fragment) = link_fragment(link) {
        let target_text = snapshot.text_for_path(&target.path)?;
        let heading = resolve_heading_fragment_text(&target_text, &fragment).unwrap_or(fragment);
        if !heading.is_empty() {
            wiki.push('#');
            wiki.push_str(&heading);
        }
    }
    let alias = text.trim();
    if !alias.is_empty() && alias != target_name {
        wiki.push('|');
        wiki.push_str(alias);
    }
    wiki.push_str("]]");
    Ok(wiki)
}

pub(in crate::state) fn markdown_url_for_target(source_path: &Path, target_path: &Path) -> String {
    let source_dir = source_path.parent().unwrap_or(source_path);
    let relative = relative_path_from(source_dir, target_path);
    percent_encode_url_path(&path_to_slash(&relative))
}

pub(in crate::state) fn relative_path_from(from_dir: &Path, target_path: &Path) -> PathBuf {
    if let Ok(stripped) = target_path.strip_prefix(from_dir)
        && !stripped.as_os_str().is_empty()
    {
        return stripped.to_path_buf();
    }

    let from_components = from_dir.components().collect::<Vec<_>>();
    let target_components = target_path.components().collect::<Vec<_>>();
    let mut common_len = 0;
    while common_len < from_components.len()
        && common_len < target_components.len()
        && from_components[common_len] == target_components[common_len]
    {
        common_len += 1;
    }

    let mut relative = PathBuf::new();
    for component in &from_components[common_len..] {
        if matches!(component, std::path::Component::Normal(_)) {
            relative.push("..");
        }
    }
    for component in &target_components[common_len..] {
        relative.push(component.as_os_str());
    }
    relative
}

pub(in crate::state) fn path_to_slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(in crate::state) fn percent_encode_url_path(path: &str) -> String {
    percent_encode_with(path, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_' | b'~')
    })
}

pub(in crate::state) fn percent_encode_url_component(component: &str) -> String {
    percent_encode_with(component, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~')
    })
}

pub(in crate::state) fn percent_encode_with(input: &str, allow: impl Fn(u8) -> bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in input.as_bytes() {
        if allow(*byte) {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

pub(crate) fn normalize_new_note_path(vault_path: &Path, candidate: impl AsRef<Path>) -> Option<PathBuf> {
    let vault_path = obsidian_core::common::normalize_path(vault_path, None);
    let path = obsidian_core::common::normalize_path(candidate, Some(&vault_path));
    if !path.starts_with(&vault_path) || path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return None;
    }
    Some(path)
}

pub(in crate::state) fn resolve_link_targets<'a>(
    source_path: &Path,
    link: &Link,
    notes: &'a [Note],
    vault_path: &Path,
) -> Vec<&'a Note> {
    match link {
        Link::Wiki { target, .. } => {
            if target.is_empty() {
                return Vec::new();
            }

            notes
                .iter()
                .filter(|note| note_matches_wiki_target(note, target))
                .collect()
        }
        Link::Markdown { url, .. } => {
            let Some(url_path) = markdown_url_path(url) else {
                return Vec::new();
            };
            if !url_path.ends_with(".md") {
                return Vec::new();
            }

            let candidates = local_markdown_candidates(source_path, &url_path, vault_path);
            notes.iter().filter(|note| candidates.contains(&note.path)).collect()
        }
        Link::Embed { .. } => Vec::new(),
    }
}

pub(in crate::state) fn note_matches_wiki_target(note: &Note, target: &str) -> bool {
    note.id == target
        || note
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem == target)
        || note.aliases.iter().any(|alias| alias == target)
}

pub(in crate::state) fn local_markdown_candidates(
    source_path: &Path,
    url_path: &str,
    vault_path: &Path,
) -> Vec<PathBuf> {
    let source_dir = source_path.parent().unwrap_or(source_path);
    let mut candidates = vec![
        obsidian_core::common::normalize_path(source_dir.join(url_path), Some(vault_path)),
        obsidian_core::common::normalize_path(url_path, Some(vault_path)),
    ];
    candidates.dedup();
    candidates
}

pub(in crate::state) fn markdown_url_path(url: &str) -> Option<String> {
    if url.contains("://") || url.starts_with('/') {
        return None;
    }

    let url_path_raw = match url.find('#') {
        Some(index) => &url[..index],
        None => url,
    };

    Some(percent_decode(url_path_raw))
}

pub(in crate::state) fn link_fragment(link: &Link) -> Option<String> {
    match link {
        Link::Wiki { heading, .. } => heading.clone(),
        Link::Markdown { url, .. } => url
            .split_once('#')
            .map(|(_, fragment)| percent_decode(fragment))
            .filter(|fragment| !fragment.is_empty()),
        Link::Embed { .. } => None,
    }
}

pub(in crate::state) fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            )
        {
            output.push((high * 16 + low) as u8);
            index += 3;
            continue;
        }

        output.push(bytes[index]);
        index += 1;
    }

    String::from_utf8(output).unwrap_or_else(|_| input.to_string())
}

pub(in crate::state) fn selected_link_fragment(link: Option<&LocatedLink>) -> Option<String> {
    link.and_then(|link| link_fragment(&link.link))
}

pub(in crate::state) fn note_target_uri(note: &Note, link: &Link) -> Result<Url, StateError> {
    let mut uri = path_to_uri(&note.path)?;
    if let Some(fragment) = link_fragment(link) {
        uri.set_fragment(Some(&fragment));
    }
    Ok(uri)
}

pub(in crate::state) fn note_location(
    snapshot: &StateSnapshot,
    note: &Note,
    fragment: Option<String>,
) -> Result<Location, StateError> {
    let mut uri = path_to_uri(&note.path)?;
    if let Some(fragment) = fragment.as_deref() {
        uri.set_fragment(Some(fragment));
    }

    Ok(Location {
        uri,
        range: note_definition_range(snapshot, note, fragment.as_deref())?,
    })
}

pub(in crate::state) fn note_definition_range(
    snapshot: &StateSnapshot,
    note: &Note,
    fragment: Option<&str>,
) -> Result<Range, StateError> {
    let text = snapshot.text_for_path(&note.path)?;

    if let Some(fragment) = fragment
        && let Some(range) = find_heading_range(&text, fragment)
    {
        return Ok(range);
    }

    if let Some(title) = note.title.as_deref()
        && let Some(range) = find_title_or_heading_range(&text, title)
    {
        return Ok(range);
    }

    if let Some(range) = find_frontmatter_key_range(&text, "id") {
        return Ok(range);
    }

    Ok(document_start_range())
}

pub(in crate::state) fn find_link_at_position(note: &Note, position: Position) -> Option<&LocatedLink> {
    note.links
        .iter()
        .filter(|link| !matches!(link.link, Link::Embed { .. }))
        .find(|link| position_in_location(position, &link.location))
}

pub(in crate::state) fn note_file_stem(note: &Note) -> String {
    note.path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(note.id.as_str())
        .to_string()
}

pub(in crate::state) fn rename_link_target_range(link: &LocatedLink) -> Option<Range> {
    let line = (link.location.line.saturating_sub(1)) as u32;
    match &link.link {
        Link::Wiki { target, .. } => {
            if target.is_empty() {
                return None;
            }
            let start = link.location.col_start + 2;
            Some(Range::new(
                Position::new(line, start as u32),
                Position::new(line, (start + target.chars().count()) as u32),
            ))
        }
        Link::Markdown { text, url } => {
            let url_start = link.location.col_start + 1 + text.chars().count() + 2;
            let path_end = url.find('#').unwrap_or(url.len());
            let raw_path = &url[..path_end];
            if raw_path.is_empty() {
                return None;
            }

            let stem_start = match (raw_path.rfind('/'), raw_path.rfind('\\')) {
                (Some(left), Some(right)) => left.max(right) + 1,
                (Some(index), None) | (None, Some(index)) => index + 1,
                (None, None) => 0,
            };
            let stem_end = raw_path
                .strip_suffix(".md")
                .map(|without_ext| without_ext.len())
                .unwrap_or(raw_path.len());
            if stem_start >= stem_end {
                return None;
            }

            let start = url_start + raw_path[..stem_start].chars().count();
            let end = url_start + raw_path[..stem_end].chars().count();
            Some(Range::new(
                Position::new(line, start as u32),
                Position::new(line, end as u32),
            ))
        }
        Link::Embed { .. } => None,
    }
}

pub(in crate::state) fn relative_display(vault_path: &Path, path: &Path) -> String {
    path.strip_prefix(vault_path).unwrap_or(path).display().to_string()
}
