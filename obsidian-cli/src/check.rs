use color_eyre::eyre;
use colored::Colorize;
use globset::{GlobBuilder, GlobSetBuilder};
use obsidian_core::Vault;

use crate::args::CheckArgs;

fn build_ignore_set(patterns: &[String]) -> eyre::Result<globset::GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| eyre::eyre!("invalid ignore pattern '{}': {}", pattern, e))?;
        builder.add(glob);
    }
    Ok(builder.build()?)
}

pub fn cmd_check(vault: Vault, args: CheckArgs) -> eyre::Result<()> {
    let ignore_set = build_ignore_set(&args.ignore)?;
    let vault_path = vault.path().to_path_buf();

    let report = vault.check(|path| {
        let rel = path.strip_prefix(&vault_path).unwrap_or(path);
        !ignore_set.is_match(rel)
    });

    println!("{} {}", "Vault root:".bold(), vault_path.display());
    println!("{} {}", "Notes:     ".bold(), report.note_count);
    println!();

    // --- Duplicate IDs ---
    if report.duplicate_ids.is_empty() {
        println!("{}", "✓ No duplicate IDs".green());
    } else {
        println!("{}", "✘ Duplicate IDs:".red().bold());
        for dup in &report.duplicate_ids {
            println!("  {}", dup.id.yellow());
            for note_ref in &dup.notes {
                let rel = note_ref.path.strip_prefix(&vault_path).unwrap_or(&note_ref.path);
                println!(
                    "    {} ({} backlinks)",
                    rel.display().to_string().cyan(),
                    note_ref.backlink_count
                );
            }
        }
    }

    // --- Duplicate aliases ---
    if report.duplicate_aliases.is_empty() {
        println!("{}", "✓ No duplicate aliases".green());
    } else {
        println!("{}", "✘ Duplicate aliases:".red().bold());
        for dup in &report.duplicate_aliases {
            println!("  {}", dup.alias.yellow());
            for note_ref in &dup.notes {
                let rel = note_ref.path.strip_prefix(&vault_path).unwrap_or(&note_ref.path);
                println!(
                    "    {} ({} backlinks)",
                    rel.display().to_string().cyan(),
                    note_ref.backlink_count
                );
            }
        }
    }

    // --- Broken links ---
    if report.broken_links.is_empty() {
        println!("{}", "✓ No broken links".green());
    } else {
        println!("{}", "✘ Broken links:".red().bold());
        let mut current_path = None;
        for broken in &report.broken_links {
            if current_path != Some(&broken.source_path) {
                current_path = Some(&broken.source_path);
                let rel = broken
                    .source_path
                    .strip_prefix(&vault_path)
                    .unwrap_or(&broken.source_path);
                println!("  {}", rel.display().to_string().cyan());
            }
            println!(
                "    {} {}",
                format!("line {}:", broken.line).dimmed(),
                broken.text.yellow()
            );
        }
    }

    if report.has_issues() {
        std::process::exit(1);
    }
    Ok(())
}
