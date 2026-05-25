# AUR packages

Two PKGBUILDs for Arch Linux users:

- `peek-git/` — builds from the latest `master` of this repo.
- `peek-bin/` — installs the prebuilt `x86_64-unknown-linux-gnu` binary from a GitHub release.

These live here for reference. The AUR itself expects each package in its own
git repository named after the package (e.g. `ssh://aur@aur.archlinux.org/peek-git.git`).

## Publishing to AUR

```sh
# One-time: register your SSH key in your AUR account.

# For peek-bin (do this after a tag is built and the GitHub release is live):
cd packaging/aur/peek-bin
updpkgsums                       # fills in sha256sums_x86_64
makepkg --printsrcinfo > .SRCINFO

git init aur-peek-bin
cp PKGBUILD .SRCINFO aur-peek-bin/
cd aur-peek-bin
git remote add origin ssh://aur@aur.archlinux.org/peek-bin.git
git add PKGBUILD .SRCINFO
git commit -m "Initial release v0.1.0"
git push -u origin master

# For peek-git:
cd packaging/aur/peek-git
makepkg --printsrcinfo > .SRCINFO
# Then publish the same way under ssh://aur@aur.archlinux.org/peek-git.git
```

## Updating peek-bin on every new release

When you tag a new version:

1. Bump `pkgver` in `peek-bin/PKGBUILD`.
2. Run `updpkgsums` to refresh the checksums.
3. Regenerate `.SRCINFO`.
4. Push to the AUR repo.

This can be automated with a separate GitHub Action — ask if you want one.

## Testing locally

```sh
cd packaging/aur/peek-git    # or peek-bin
makepkg -si                  # builds and installs
```
