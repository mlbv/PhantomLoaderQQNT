<!-- SPDX-FileCopyrightText: 2026 movblock -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Release verification

The Windows x64 release ZIP is compiled from the tagged source on GitHub-hosted Windows runners and includes a SHA-256 file, `build-info.json`, and `provenance.jsonl`. Corresponding source is available through GitHub's automatic **Source code (zip)** and **Source code (tar.gz)** downloads for the same signed tag.

PGP authenticates the annotated release tag and its target commit. GitHub's provenance attestation binds the downloaded archive's SHA-256 to the workflow, source commit and tag. The private PGP key remains with the maintainer; GitHub Actions only receives the public key. The signing primary fingerprint is:

```text
55BD 0E0B E592 652E D242 FD84 9ED1 BD31 3EFD 5297
```

The public key is [included in the source](../.github/keys/release.asc) and available at <https://github.com/mlbv.gpg> or through public keyservers under `admin@movblock.mov`. Compare the full primary fingerprint before trusting an imported key. Signing subkeys must belong to this primary key.

## Verify a download

Use Git, GnuPG and the GitHub CLI from a fresh clone. Before running any repository helper, compare the public key's primary fingerprint with the value above, import the matching key, and verify both signatures using Git. The bundled key is data; no repository script needs to run for these checks:

```powershell
gpg --show-keys --with-fingerprint .github/keys/release.asc
# Check the full primary fingerprint before importing.
gpg --import .github/keys/release.asc
$tag = 'v1.0.0' # Replace with the release being verified.
git fetch origin "refs/tags/${tag}:refs/tags/${tag}"
git -c gpg.format=openpgp verify-tag --raw $tag
$commit = git --no-replace-objects rev-parse "${tag}^{commit}"
git -c gpg.format=openpgp verify-commit --raw $commit
```

Both verifications must succeed. Their `[GNUPG:] VALIDSIG` records must name the expected primary fingerprint (the last field for signing-subkey signatures); reject invalid, expired or revoked signatures and other signers. Once the source is authenticated, the helper below automates the stricter release checks. Run it from a checkout of that verified commit. Download the Windows x64 ZIP and its `.sha256` file into the current directory.

```powershell
$tag = 'v1.0.0' # Replace with the release being verified.
git fetch origin "refs/tags/${tag}:refs/tags/${tag}"
if ($LASTEXITCODE) { throw 'Tag fetch failed.' }
$commit = git rev-parse "${tag}^{commit}"
if ($LASTEXITCODE) { throw 'Cannot resolve the release commit.' }
pwsh -File .github/scripts/verify-release.ps1 -Tag $tag -ExpectedCommit $commit
if ($LASTEXITCODE) { throw 'PGP verification failed.' }

$checksums = @(Get-ChildItem 'PhantomLoaderQQNT-*.zip.sha256')
if ($checksums.Count -ne 1) { throw 'Keep one release checksum file in this directory.' }
foreach ($checksum in $checksums) {
    $expected, $file = (Get-Content $checksum -Raw).Trim() -split '  ', 2
    if ((Get-FileHash $file -Algorithm SHA256).Hash.ToLowerInvariant() -cne $expected) {
        throw "SHA-256 mismatch: $file"
    }
    gh attestation verify $file --repo mlbv/PhantomLoaderQQNT `
        --cert-identity 'https://github.com/mlbv/PhantomLoaderQQNT/.github/workflows/publish-release.yml@refs/heads/main' `
        --source-ref "refs/tags/$tag" --source-digest $commit `
        --deny-self-hosted-runners
    if ($LASTEXITCODE) { throw "Provenance verification failed: $file" }
}
```

The signature helper imports the bundled key into a temporary keyring and checks its primary fingerprint. It rejects unsigned tags, unsigned commits, other signing keys, expired or revoked signatures, and tags whose target differs from the expected commit. It verifies the release tag directly targets that commit, rather than another tag.

The attestation can also be verified using the downloaded `provenance.jsonl` by adding `--bundle provenance.jsonl`. Verification still requires Sigstore's trusted root material; the bundle alone is not a trust root. `build-info.json` records the exact source commit, Rust/Cargo/Node versions, runner image version, profile hash and archive hashes.

These checks establish the archive's source and build origin. They do not establish that the source is safe or that a separate rebuild will be byte-identical. Hosted runner images, MSVC and archive timestamps can change. The release build pins its Rust and Node versions, Cargo lockfile, upstream archive hashes, and action commit IDs; it does not restore compiled build caches.

To rebuild from GitHub's source download, open the extracted repository directory and follow [the build instructions](distribution.md#building-the-bundle). Fetching pinned dependencies requires network access unless they are already cached.
