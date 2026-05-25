# AUR packages

Two PKGBUILDs for Arch Linux users:

- `ipeek-git/` — builds from the latest `master` of this repo.
- `ipeek-bin/` — installs the prebuilt `x86_64-unknown-linux-gnu` binary from a GitHub release.

The binary is named `ipeek` (not `peek`) because the `peek` name on AUR is
already taken by an unrelated GIF screen recorder. The GitHub repo stays
`prietus/peek`; only the installed binary and the AUR package names differ.

These live here for reference. The AUR itself expects each package in its own
git repository named after the package (e.g. `ssh://aur@aur.archlinux.org/ipeek-git.git`).

## Publishing to AUR

```sh
# One-time: register your SSH key in your AUR account.

# For ipeek-bin (do this after a tag is built and the GitHub release is live):
cd packaging/aur/ipeek-bin
updpkgsums                       # fills in sha256sums_x86_64
makepkg --printsrcinfo > .SRCINFO

git init aur-ipeek-bin
cp PKGBUILD .SRCINFO aur-ipeek-bin/
cd aur-ipeek-bin
git remote add origin ssh://aur@aur.archlinux.org/ipeek-bin.git
git add PKGBUILD .SRCINFO
git commit -m "Initial release v0.1.0"
git push -u origin master

# For ipeek-git:
cd packaging/aur/ipeek-git
makepkg --printsrcinfo > .SRCINFO
# Then publish the same way under ssh://aur@aur.archlinux.org/ipeek-git.git
```

## Updating ipeek-bin on every new release

When you tag a new version:

1. Bump `pkgver` in `ipeek-bin/PKGBUILD`.
2. Run `updpkgsums` to refresh the checksums.
3. Regenerate `.SRCINFO`.
4. Push to the AUR repo.

This can be automated with a separate GitHub Action — ask if you want one.

## Testing locally

```sh
cd packaging/aur/ipeek-git    # or ipeek-bin
makepkg -si                  # builds and installs
```
