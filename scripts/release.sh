#!/usr/bin/env bash

set -e

# Ensure prerequisites are installed
cargo install cargo-set-version

# Check latest published version
LATEST_VERSION=$(cargo search obsidian-rs-core 2>/dev/null | grep 'obsidian-rs-core =' | awk -F'"' '{print $2}')
echo "Latest published version: $LATEST_VERSION"

# Resolve version to publish
LOCAL_VERSION=$(cargo metadata --format-version=1 --no-deps | jq -r '.packages[0].version')
if [[ "$LOCAL_VERSION" != "$LATEST_VERSION" ]]; then
    echo "Local version is:         $LOCAL_VERSION"
    read -rp "Continue with release as version ${LOCAL_VERSION}? [Y/n] " prompt
    if [[ $prompt == "y" || $prompt == "Y" || $prompt == "yes" || $prompt == "Yes" ]]; then
        VERSION="$LOCAL_VERSION"
    else
        echo "Canceled"
        exit 1
    fi
else
    read -rp "Enter version to publish: " VERSION
    if [[ -z "$VERSION" ]]; then
        echo "Version cannot be empty"
        exit 1
    fi
    cargo set-version "$VERSION"
fi

# Ensure changelog has section for new version
if ! grep -q "^## v$VERSION" CHANGELOG.md; then
    echo "Updating changelog..."
    sed -i '' "s/## Unreleased/## Unreleased\n\n## v$VERSION - $(date +%Y-%m-%d)/" CHANGELOG.md
    echo "========================================================================================="
    git diff CHANGELOG.md
    echo "========================================================================================="
    echo ""
else
    echo "Changelog already has section for version $VERSION"
fi

# Pull out release notes for new version
release_notes_start=$(grep -n "^## v$VERSION" CHANGELOG.md | cut -d: -f1)
release_notes_end=$(grep -n "^## " CHANGELOG.md | grep -v ":## Unreleased" | grep -v ":## v$VERSION" | head -n 1 | cut -d: -f1)
release_notes=$(head -n $((release_notes_end - 1)) CHANGELOG.md | tail -n +$((release_notes_start + 1)))
release_notes=$(printf "%s" "$release_notes" | gsed -z 's/^[[:space:]]*//;s/[[:space:]]*$//')
echo "Release notes for version $VERSION:"
echo "========================================================================================="
echo "$release_notes"
echo "========================================================================================="
echo ""
read -rp "Does this look good? [Y/n] " prompt
if ! [[ $prompt == "y" || $prompt == "Y" || $prompt == "yes" || $prompt == "Yes" ]]; then echo "Canceled"
    git checkout CHANGELOG.md
    exit 1
fi

# Commit changes
echo "Committing changes..."
git add -A
git commit -m "(chore) prepare for release $VERSION" || true && git push

# Publish all workspace crates
echo "Publishing all workspace crates..."
cargo publish --workspace

# Create git tag
echo "Creating new git tag v$VERSION"
git tag "v$VERSION" -m "Release $VERSION"
git push --tags

# Publish gh release from tag
gh release create "v$VERSION" --verify-tag --notes "$release_notes"
