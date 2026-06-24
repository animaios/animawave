#!/bin/sh
# deploy-aur.sh — Push code changes to GitHub and update AUR package
#
# Prerequisites:
#   1. SSH key configured for aur@aur.archlinux.org
#   2. AUR repo cloned: git clone ssh://aur@aur.archlinux.org/animawave-git.git
#   3. Git credentials configured for the animawave GitHub repo
#
# Usage:
#   ./deploy-aur.sh          # commit, push to GitHub, update AUR
#   ./deploy-aur.sh --aur-only   # only update AUR PKGBUILD

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info()  { printf "${GREEN}✓${NC} %s\n" "$1"; }
warn()  { printf "${YELLOW}⚠${NC} %s\n" "$1"; }
error() { printf "${RED}✗${NC} %s\n" "$1"; }

# ── Config ──────────────────────────────────────────────────────────
AUR_REPO="${AUR_REPO:-$HOME/aur/animawave-git}"
GIT_REMOTE="origin"
AUR_REMOTE="aur"

# ── Step 1: Check working tree is clean ─────────────────────────────
if [ -n "$(git status --porcelain)" ]; then
    warn "Uncommitted changes detected."
    echo "  Changes to commit:"
    git status --short
    echo ""
    echo "Committing all changes..."
    git add -A
    git commit -m "5.0.1: network resilience improvements

- Retry API requests with exponential backoff on transient errors
- Add DNS fallback servers when standard discovery fails
- Improve TCP connect/keepalive settings for unstable networks
- Add configurable retry count, request timeout, connect timeout"
    info "Committed."
else
    info "Working tree clean, nothing new to commit."
fi

# ── Step 2: Tag release ─────────────────────────────────────────────
CURRENT_TAG=$(git describe --tags --abbrev=0 2>/dev/null || echo "none")
if [ "$CURRENT_TAG" != "5.0.1" ]; then
    git tag -a "5.0.1" -m "5.0.1 — Network resilience improvements"
    info "Created tag 5.0.1"
else
    info "Tag 5.0.1 already exists"
fi

# ── Step 3: Push to GitHub ──────────────────────────────────────────
echo ""
echo "Pushing to GitHub ($GIT_REMOTE)..."
git push "$GIT_REMOTE" main --tags
info "Pushed to GitHub"

# ── Step 4: Update AUR package ──────────────────────────────────────
if [ ! -d "$AUR_REPO" ]; then
    warn "AUR repo not found at $AUR_REPO"
    echo "  Cloning AUR repo..."
    git clone "ssh://aur@aur.archlinux.org/animawave-git.git" "$AUR_REPO"
    info "Cloned AUR repo"
fi

echo ""
echo "Updating AUR PKGBUILD in $AUR_REPO..."
cp PKGBUILD "$AUR_REPO/"
cp .SRCINFO "$AUR_REPO/"

cd "$AUR_REPO"
if [ -n "$(git status --porcelain)" ]; then
    git add PKGBUILD .SRCINFO
    git commit -m "Update to 5.0.1

- Network resilience: exponential backoff retry, DNS fallback,
  improved TCP keepalive/connect timeout, configurable settings.
- Use proper pkgver() function for git-based versioning."
    git push "$AUR_REMOTE" master
    info "Pushed to AUR"
else
    info "No PKGBUILD changes to push to AUR"
fi

echo ""
echo "────────────────────────────────────────────"
info "Deployment complete!"
echo "  GitHub: https://github.com/animaios/animawave"
echo "  AUR:    https://aur.archlinux.org/packages/animawave-git"
echo ""
echo "Users can update with:"
echo "  yay -Syu animawave-git"
echo "  # or: paru -Syu animawave-git"
echo "────────────────────────────────────────────"

# Clean up the PKGBUILD/.SRCINFO copies we placed in the project (optional)
cd "$OLDPWD"
rm -f PKGBUILD .SRCINFO deploy-aur.sh
info "Cleaned up AUR packaging files from project root"
