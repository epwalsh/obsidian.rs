use super::*;
use obsidian_core::StrandedNote;

#[derive(Clone, Debug)]
pub struct DiagnosticUpdate {
    pub uri: Url,
    pub version: Option<i32>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub struct DiagnosticsBatch {
    pub revision: u64,
    pub updates: Vec<DiagnosticUpdate>,
    pub published_diagnostics: HashMap<PathBuf, Url>,
}

#[derive(Debug)]
pub struct DiagnosticsRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) previously_published: HashMap<PathBuf, Url>,
    pub(in crate::state) primary_document: Option<PrimaryDocument>,
    pub(in crate::state) revision: u64,
}

#[derive(Debug)]
pub(in crate::state) struct PrimaryDocument {
    pub(in crate::state) path: PathBuf,
    pub(in crate::state) uri: Url,
    pub(in crate::state) version: Option<i32>,
}

impl DiagnosticsRequest {
    pub fn compute(self) -> Result<DiagnosticsBatch, StateError> {
        let ignore_set = build_ignore_set(&self.snapshot.diagnostics_ignore);
        let notes = self.snapshot.notes();
        let visible_notes = notes
            .into_iter()
            .filter(|note| {
                let path = &note.path;
                let rel = path.strip_prefix(self.snapshot.vault_path.as_path()).unwrap_or(path);
                !ignore_set.is_match(rel)
            })
            .collect::<Vec<_>>();
        let report = obsidian_core::check_notes(&self.snapshot.vault_path, &visible_notes);
        let mut diagnostics_by_path = build_diagnostics_by_path(&self.snapshot, &report)?;

        let mut paths_to_publish = BTreeSet::new();
        paths_to_publish.extend(diagnostics_by_path.keys().cloned());
        paths_to_publish.extend(self.previously_published.keys().cloned());
        if let Some(primary_document) = &self.primary_document {
            paths_to_publish.insert(primary_document.path.clone());
        }

        let mut published_diagnostics = HashMap::new();
        let mut updates = Vec::with_capacity(paths_to_publish.len());

        for path in paths_to_publish {
            let uri = if self
                .primary_document
                .as_ref()
                .is_some_and(|primary_document| path == primary_document.path)
            {
                self.primary_document.as_ref().unwrap().uri.clone()
            } else {
                self.snapshot.uri_for_path(&path)?
            };
            let version = if self
                .primary_document
                .as_ref()
                .is_some_and(|primary_document| path == primary_document.path)
            {
                self.primary_document.as_ref().unwrap().version
            } else {
                self.snapshot.version_for_path(&path)
            };
            let diagnostics = diagnostics_by_path.remove(&path).unwrap_or_default();

            if !diagnostics.is_empty() {
                published_diagnostics.insert(path.clone(), uri.clone());
            }

            updates.push(DiagnosticUpdate {
                uri,
                version,
                diagnostics,
            });
        }

        Ok(DiagnosticsBatch {
            revision: self.revision,
            updates,
            published_diagnostics,
        })
    }
}

pub(in crate::state) fn build_diagnostics_by_path(
    snapshot: &StateSnapshot,
    report: &obsidian_core::VaultHealthReport,
) -> Result<HashMap<PathBuf, Vec<Diagnostic>>, StateError> {
    let mut diagnostics_by_path: HashMap<PathBuf, Vec<Diagnostic>> = HashMap::new();

    for duplicate in &report.duplicate_ids {
        add_duplicate_id_diagnostics(snapshot, &mut diagnostics_by_path, duplicate)?;
    }

    for duplicate in &report.duplicate_aliases {
        add_duplicate_alias_diagnostics(snapshot, &mut diagnostics_by_path, duplicate)?;
    }

    add_broken_link_diagnostics(snapshot, &mut diagnostics_by_path, &report.broken_links)?;
    add_stranded_note_diagnostics(&mut diagnostics_by_path, &report.stranded_notes);

    for diagnostics in diagnostics_by_path.values_mut() {
        diagnostics.sort_by(|left, right| {
            left.range
                .start
                .line
                .cmp(&right.range.start.line)
                .then(left.range.start.character.cmp(&right.range.start.character))
                .then(left.message.cmp(&right.message))
        });
    }

    Ok(diagnostics_by_path)
}

pub(in crate::state) fn add_duplicate_id_diagnostics(
    snapshot: &StateSnapshot,
    diagnostics_by_path: &mut HashMap<PathBuf, Vec<Diagnostic>>,
    duplicate: &DuplicateId,
) -> Result<(), StateError> {
    for note in &duplicate.notes {
        let other_paths = duplicate
            .notes
            .iter()
            .filter(|candidate| candidate.path != note.path)
            .map(|candidate| relative_display(snapshot.vault_path.as_path(), &candidate.path))
            .collect::<Vec<_>>();

        diagnostics_by_path
            .entry(note.path.clone())
            .or_default()
            .push(make_diagnostic(
                duplicate_id_range(snapshot, &note.path)?,
                "duplicate-id",
                format!(
                    "Duplicate note ID `{}` also used by {}.",
                    duplicate.id,
                    other_paths.join(", ")
                ),
            ));
    }

    Ok(())
}

pub(in crate::state) fn add_duplicate_alias_diagnostics(
    snapshot: &StateSnapshot,
    diagnostics_by_path: &mut HashMap<PathBuf, Vec<Diagnostic>>,
    duplicate: &DuplicateAlias,
) -> Result<(), StateError> {
    for note in &duplicate.notes {
        let other_paths = duplicate
            .notes
            .iter()
            .filter(|candidate| candidate.path != note.path)
            .map(|candidate| relative_display(snapshot.vault_path.as_path(), &candidate.path))
            .collect::<Vec<_>>();

        diagnostics_by_path
            .entry(note.path.clone())
            .or_default()
            .push(make_diagnostic(
                duplicate_alias_range(snapshot, &note.path, &duplicate.alias)?,
                "duplicate-alias",
                format!(
                    "Duplicate alias `{}` also used by {}.",
                    duplicate.alias,
                    other_paths.join(", ")
                ),
            ));
    }

    Ok(())
}

pub(in crate::state) fn add_stranded_note_diagnostics(
    diagnostics_by_path: &mut HashMap<PathBuf, Vec<Diagnostic>>,
    stranded_notes: &[StrandedNote],
) {
    for stranded in stranded_notes {
        diagnostics_by_path
            .entry(stranded.path.clone())
            .or_default()
            .push(make_diagnostic(
                document_start_range(),
                "stranded-note",
                "Stranded note has no incoming or outgoing note links.".to_string(),
            ));
    }
}

pub(in crate::state) fn add_broken_link_diagnostics(
    snapshot: &StateSnapshot,
    diagnostics_by_path: &mut HashMap<PathBuf, Vec<Diagnostic>>,
    broken_links: &[BrokenLink],
) -> Result<(), StateError> {
    let mut note_cache: HashMap<PathBuf, Note> = HashMap::new();
    let mut used_ranges: HashMap<PathBuf, Vec<Range>> = HashMap::new();

    for broken in broken_links {
        let note = match note_cache.get(&broken.source_path) {
            Some(note) => note,
            None => {
                let note = snapshot.note_for_path(&broken.source_path)?;
                note_cache.insert(broken.source_path.clone(), note);
                note_cache.get(&broken.source_path).unwrap()
            }
        };
        let range = find_broken_link_range(note, broken, used_ranges.entry(broken.source_path.clone()).or_default())
            .unwrap_or_else(|| line_start_range(broken.line));

        diagnostics_by_path
            .entry(broken.source_path.clone())
            .or_default()
            .push(make_diagnostic(
                range,
                "broken-link",
                format!("Broken link {}.", broken.text),
            ));
    }

    Ok(())
}

pub(in crate::state) fn find_broken_link_range(
    note: &Note,
    broken: &BrokenLink,
    used_ranges: &mut Vec<Range>,
) -> Option<Range> {
    for link in &note.links {
        let range = location_to_range(&link.location);
        if used_ranges.contains(&range) {
            continue;
        }
        if link.location.line != broken.line || !link_matches_broken(link, broken) {
            continue;
        }

        used_ranges.push(range);
        return Some(range);
    }

    None
}

pub(in crate::state) fn link_matches_broken(link: &obsidian_core::LocatedLink, broken: &BrokenLink) -> bool {
    match &link.link {
        Link::Wiki { target, .. } => broken.text == format!("[[{}]]", target),
        Link::Markdown { url, .. } => broken.text == format!("[...]({})", url),
        Link::Embed { .. } => false,
    }
}

pub(in crate::state) fn duplicate_id_range(snapshot: &StateSnapshot, path: &Path) -> Result<Range, StateError> {
    let text = snapshot.text_for_path(path)?;
    Ok(find_frontmatter_key_range(&text, "id").unwrap_or_else(document_start_range))
}

pub(in crate::state) fn duplicate_alias_range(
    snapshot: &StateSnapshot,
    path: &Path,
    alias: &str,
) -> Result<Range, StateError> {
    let text = snapshot.text_for_path(path)?;
    if let Some(range) = frontmatter_alias_ranges(&text)
        .into_iter()
        .find(|alias_range| alias_range.value.eq_ignore_ascii_case(alias))
        .map(|alias_range| alias_range.range)
    {
        return Ok(range);
    }

    let note = snapshot.note_for_path(path)?;
    if let Some(title) = note.title.as_deref()
        && title.eq_ignore_ascii_case(alias)
        && let Some(range) = find_title_or_heading_range(&text, title)
    {
        return Ok(range);
    }

    Ok(find_alias_range(&text, alias).unwrap_or_else(document_start_range))
}
