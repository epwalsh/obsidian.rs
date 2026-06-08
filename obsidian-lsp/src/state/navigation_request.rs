use super::*;

#[derive(Debug)]
pub struct NavigationRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) path: PathBuf,
    pub(in crate::state) position: Position,
}

pub(in crate::state) struct NavigationContext {
    snapshot: StateSnapshot,
    notes: Vec<Note>,
    source_note: Note,
    selected_link: Option<LocatedLink>,
    selected_tag: Option<TagSelection>,
}

impl NavigationRequest {
    pub fn compute_hover(self) -> Result<Option<Hover>, StateError> {
        let context = self.build_context()?;
        if let Some(selected_tag) = context.selected_tag.as_ref() {
            let occurrences = tag_occurrences(&context.snapshot, &selected_tag.tag)?;
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: render_tag_hover(&selected_tag.tag, occurrences.len()),
                }),
                range: Some(selected_tag.range),
            }));
        }

        let Some(selected_link) = context.selected_link.as_ref() else {
            return Ok(None);
        };
        let matching_notes = context.resolve_selected_link_targets();

        if matching_notes.is_empty() {
            return Ok(None);
        }

        let contents = if matching_notes.len() == 1 {
            render_note_hover(matching_notes[0], context.snapshot.vault_path.as_path())
        } else {
            render_ambiguous_hover(&matching_notes, context.snapshot.vault_path.as_path())
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: contents,
            }),
            range: Some(location_to_range(&selected_link.location)),
        }))
    }

    pub fn compute_references(self) -> Result<Option<Vec<Location>>, StateError> {
        let context = self.build_context()?;
        if let Some(selected_tag) = context.selected_tag.as_ref() {
            return Ok(Some(tag_locations(&context.snapshot, &selected_tag.tag)?));
        }

        let target_notes = if context.selected_link.is_some() {
            let matching = context.resolve_selected_link_targets();
            if matching.len() != 1 {
                return Ok(Some(Vec::new()));
            }
            vec![matching[0]]
        } else {
            // Obsidian-specific behavior: when the cursor is not on a link, treat
            // references as backlinks to the current note.
            vec![&context.source_note]
        };

        let mut locations = Vec::new();
        for target in target_notes {
            for (source_note, links) in
                obsidian_core::backlinks_from(&context.notes, target, context.snapshot.vault_path.as_path())
            {
                let uri = context.snapshot.uri_for_path(&source_note.path)?;
                locations.extend(links.into_iter().map(|link| Location {
                    uri: uri.clone(),
                    range: location_to_range(&link.location),
                }));
            }
        }

        locations.sort_by(|left, right| {
            left.uri
                .cmp(&right.uri)
                .then(left.range.start.line.cmp(&right.range.start.line))
                .then(left.range.start.character.cmp(&right.range.start.character))
        });
        locations.dedup_by(|left, right| left.uri == right.uri && left.range == right.range);

        Ok(Some(locations))
    }

    pub fn compute_definition(self) -> Result<Option<GotoDefinitionResponse>, StateError> {
        let context = self.build_context()?;
        if let Some(selected_tag) = context.selected_tag.as_ref() {
            let mut locations = tag_locations(&context.snapshot, &selected_tag.tag)?;
            return Ok(if locations.is_empty() {
                None
            } else if locations.len() == 1 {
                Some(GotoDefinitionResponse::Scalar(locations.remove(0)))
            } else {
                Some(GotoDefinitionResponse::Array(locations))
            });
        }

        let matching_notes = context.resolve_selected_link_targets();

        if matching_notes.is_empty() {
            // Anchor-only wiki link like [[#Heading]] — navigate within the current document.
            if let Some(LocatedLink {
                link:
                    Link::Wiki {
                        target,
                        heading: Some(_),
                        ..
                    },
                ..
            }) = &context.selected_link
                && target.is_empty()
            {
                let fragment = selected_link_fragment(context.selected_link.as_ref());
                let location = note_location(&context.snapshot, &context.source_note, fragment)?;
                return Ok(Some(GotoDefinitionResponse::Scalar(location)));
            }
            return Ok(None);
        }

        let mut locations = matching_notes
            .into_iter()
            .map(|note| {
                note_location(
                    &context.snapshot,
                    note,
                    selected_link_fragment(context.selected_link.as_ref()),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        locations.sort_by(|left, right| {
            left.uri
                .cmp(&right.uri)
                .then(left.range.start.line.cmp(&right.range.start.line))
                .then(left.range.start.character.cmp(&right.range.start.character))
        });

        Ok(Some(if locations.len() == 1 {
            GotoDefinitionResponse::Scalar(locations.remove(0))
        } else {
            GotoDefinitionResponse::Array(locations)
        }))
    }

    fn build_context(self) -> Result<NavigationContext, StateError> {
        let notes = self.snapshot.notes();
        let source_note = self.snapshot.note_for_path(&self.path)?;
        let selected_link = find_link_at_position(&source_note, self.position).cloned();
        let selected_tag = if selected_link.is_none() {
            find_tag_at_position(&self.snapshot, &source_note, self.position)?
        } else {
            None
        };
        Ok(NavigationContext {
            snapshot: self.snapshot,
            notes,
            source_note,
            selected_link,
            selected_tag,
        })
    }
}

impl NavigationContext {
    fn resolve_selected_link_targets(&self) -> Vec<&Note> {
        self.selected_link
            .as_ref()
            .map(|link| {
                resolve_link_targets(
                    &self.source_note.path,
                    &link.link,
                    &self.notes,
                    self.snapshot.vault_path.as_path(),
                )
            })
            .unwrap_or_default()
    }
}

pub(in crate::state) fn render_note_hover(note: &Note, vault_path: &Path) -> String {
    let mut lines = Vec::new();
    let heading = note.title.as_deref().unwrap_or(note.id.as_str());
    lines.push(format!("**{}**", heading));
    lines.push(String::new());
    lines.push(format!("- Path: `{}`", relative_display(vault_path, &note.path)));
    lines.push(format!("- ID: `{}`", note.id));

    if !note.aliases.is_empty() {
        lines.push(format!("- Aliases: {}", markdown_list(note.aliases.iter().cloned())));
    }

    let tags = note
        .tags
        .iter()
        .map(|tag| tag.tag.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !tags.is_empty() {
        lines.push(format!("- Tags: {}", markdown_list(tags)));
    }

    lines.join("\n")
}

pub(in crate::state) fn render_tag_hover(tag: &str, occurrence_count: usize) -> String {
    format!("**#{}**\n\n- Occurrences: {}", tag, occurrence_count)
}

pub(in crate::state) fn render_ambiguous_hover(notes: &[&Note], vault_path: &Path) -> String {
    let mut lines = vec![
        "**Ambiguous note link**".to_string(),
        String::new(),
        "This link matches more than one note:".to_string(),
        String::new(),
    ];

    for note in notes {
        lines.push(format!("- `{}`", relative_display(vault_path, &note.path)));
    }

    lines.join("\n")
}

pub(in crate::state) fn markdown_list(items: impl IntoIterator<Item = String>) -> String {
    items
        .into_iter()
        .map(|item| format!("`{}`", item))
        .collect::<Vec<_>>()
        .join(", ")
}
