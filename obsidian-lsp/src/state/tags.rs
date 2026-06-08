use super::*;

#[derive(Clone, Debug)]
pub(in crate::state) struct TagSelection {
    pub(in crate::state) tag: String,
    pub(in crate::state) range: Range,
    pub(in crate::state) rename_range: Range,
    pub(in crate::state) placeholder: String,
}
#[derive(Clone, Debug)]
pub(in crate::state) struct TagOccurrence {
    pub(in crate::state) path: PathBuf,
    pub(in crate::state) range: Range,
    pub(in crate::state) inline: bool,
}
pub(in crate::state) fn find_tag_at_position(
    snapshot: &StateSnapshot,
    note: &Note,
    position: Position,
) -> Result<Option<TagSelection>, StateError> {
    for tag in &note.tags {
        if let CoreLocation::Inline(location) = &tag.location
            && position_in_location(position, location)
        {
            let range = location_to_range(location);
            return Ok(Some(TagSelection {
                tag: tag.tag.clone(),
                rename_range: range,
                range,
                placeholder: format!("#{}", tag.tag),
            }));
        }
    }

    let frontmatter_tags = note
        .tags
        .iter()
        .filter_map(|tag| match tag.location {
            CoreLocation::Frontmatter => Some(tag.tag.as_str()),
            CoreLocation::Inline(_) => None,
        })
        .collect::<Vec<_>>();
    if frontmatter_tags.is_empty() {
        return Ok(None);
    }

    let text = snapshot.text_for_path(&note.path)?;
    for tag_range in frontmatter_tag_ranges(&text) {
        if !frontmatter_tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(&tag_range.tag))
        {
            continue;
        }
        if position_in_range(position, &tag_range.range) {
            return Ok(Some(TagSelection {
                tag: tag_range.tag.clone(),
                range: tag_range.range,
                rename_range: tag_range.range,
                placeholder: tag_range.tag,
            }));
        }
    }

    Ok(None)
}

pub(in crate::state) fn tag_locations(snapshot: &StateSnapshot, tag: &str) -> Result<Vec<Location>, StateError> {
    let mut locations = tag_occurrences(snapshot, tag)?
        .into_iter()
        .map(|occurrence| {
            Ok(Location {
                uri: snapshot.uri_for_path(&occurrence.path)?,
                range: occurrence.range,
            })
        })
        .collect::<Result<Vec<_>, StateError>>()?;

    locations.sort_by(|left, right| {
        left.uri
            .cmp(&right.uri)
            .then(left.range.start.line.cmp(&right.range.start.line))
            .then(left.range.start.character.cmp(&right.range.start.character))
    });
    locations.dedup_by(|left, right| left.uri == right.uri && left.range == right.range);
    Ok(locations)
}

pub(in crate::state) fn tag_occurrences(snapshot: &StateSnapshot, tag: &str) -> Result<Vec<TagOccurrence>, StateError> {
    let notes = snapshot.notes();
    let mut occurrences = Vec::new();
    let mut frontmatter_cache: HashMap<PathBuf, Vec<FrontmatterTagRange>> = HashMap::new();
    let mut used_frontmatter_ranges: HashMap<PathBuf, Vec<Range>> = HashMap::new();

    for note in notes {
        let matching_tags = note
            .tags
            .iter()
            .filter(|candidate| tag_matches_query_tag(&candidate.tag, tag))
            .cloned()
            .collect::<Vec<_>>();
        for located_tag in matching_tags {
            match located_tag.location {
                CoreLocation::Inline(location) => occurrences.push(TagOccurrence {
                    path: note.path.clone(),
                    range: location_to_range(&location),
                    inline: true,
                }),
                CoreLocation::Frontmatter => {
                    let ranges = match frontmatter_cache.get(&note.path) {
                        Some(ranges) => ranges,
                        None => {
                            let text = snapshot.text_for_path(&note.path)?;
                            frontmatter_cache.insert(note.path.clone(), frontmatter_tag_ranges(&text));
                            frontmatter_cache.get(&note.path).unwrap()
                        }
                    };
                    let used = used_frontmatter_ranges.entry(note.path.clone()).or_default();
                    if let Some(tag_range) = ranges.iter().find(|tag_range| {
                        tag_range.tag.eq_ignore_ascii_case(&located_tag.tag) && !used.contains(&tag_range.range)
                    }) {
                        used.push(tag_range.range);
                        occurrences.push(TagOccurrence {
                            path: note.path.clone(),
                            range: tag_range.range,
                            inline: false,
                        });
                    }
                }
            }
        }
    }

    occurrences.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.range.start.line.cmp(&right.range.start.line))
            .then(left.range.start.character.cmp(&right.range.start.character))
    });
    Ok(occurrences)
}

pub(in crate::state) fn tag_matches_query_tag(candidate: &str, query: &str) -> bool {
    candidate.eq_ignore_ascii_case(query)
        || candidate
            .to_lowercase()
            .starts_with(&format!("{}/", query.to_lowercase()))
}
