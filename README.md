# CodeZ

CodeZ is a native macOS app for browsing and editing code, reviewing git diffs,
and running Codex or Claude Code sessions from one window.

## Build and Run

```sh
cargo run
cargo run -- <path>
cargo build --release
```

`cargo run -- <path>` accepts either a folder or a file. When given a file, CodeZ
opens its parent folder and selects the file.

## Website

The static website lives in `website/` and is intended to be deployed to:

```txt
https://codez.elsetech.app
```

The app checks this site for updates by fetching:

```txt
https://codez.elsetech.app/update.json
```

The manifest shape is:

```json
{
  "version": "0.1.0",
  "download_url": "https://codez.elsetech.app/downloads/CodeZ-latest.dmg",
  "release_notes_url": "https://codez.elsetech.app/#install",
  "notes": "A new CodeZ build is available."
}
```

If `version` is newer than the app's compiled `Cargo.toml` version, CodeZ shows
an update prompt and opens `download_url` when the user clicks Download.

## Release Flow

1. Update the package version in `Cargo.toml`.
2. Update `website/update.json` to the same version.
3. Make sure the signing identity is available:

```sh
security find-identity -v -p codesigning
```

`scripts/package-macos.sh` prefers a `Developer ID Application` certificate. If
none is installed, it falls back to an `Apple Development` certificate for
local/test builds. Public downloads should use Developer ID signing and
notarization. When a Developer ID certificate is used, the script requires
`CODEZ_NOTARY_PROFILE`; this prevents publishing a DMG that macOS rejects as
unnotarized.

To force a specific identity:

```sh
export CODEZ_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
```

To notarize during packaging, create an App Store Connect API key:

1. Open `https://appstoreconnect.apple.com/access/integrations/api`.
2. Go to `Users and Access` → `Integrations` → `App Store Connect API` → `Keys`.
3. Create a key such as `CodeZ Notary`.
4. Download the `.p8` private key. It can only be downloaded once.
5. Record the Key ID and Issuer ID from App Store Connect.

Store those credentials in the keychain once:

```sh
xcrun notarytool store-credentials codez-notary \
  --key ~/Keys/AuthKey_<KEY_ID>.p8 \
  --key-id <KEY_ID> \
  --issuer <ISSUER_ID>
```

If App Store Connect gives you an individual API key and `notarytool` rejects
`--issuer`, retry the same command without `--issuer`.

Then enable notarization for packaging:

```sh
export CODEZ_NOTARY_PROFILE=codez-notary
```

4. Run:

```sh
./scripts/package-macos.sh
```

The packaging script builds the universal macOS app, creates the DMG, and copies
the DMG into:

```txt
website/downloads/CodeZ-latest.dmg
website/downloads/CodeZ-<version>.dmg
```

5. Deploy the full `website/` directory to `https://codez.elsetech.app`.
6. Verify these URLs after deployment:

```txt
https://codez.elsetech.app/
https://codez.elsetech.app/update.json
https://codez.elsetech.app/downloads/CodeZ-latest.dmg
```

## Checks

Before shipping app changes:

```sh
cargo test updater
cargo build
```
