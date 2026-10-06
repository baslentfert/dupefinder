# Duplicate Finder

A Windows tray app that finds duplicate files, look-alike photos and orphaned
leftovers, and helps you decide what to do with them.

Tauri 2 shell, Svelte 5 + TypeScript front end, Rust engine. MIT licensed.

- **Byte-exact duplicates**, found in four stages so most files are ruled out
  without reading a byte of their contents.
- **Look-alike photos** — resizes, re-encodes, messaging-app copies and edits
  of the same picture, including iPhone HEIC.
- **Duplicated folders**, so "I have this folder six times" is one decision
  instead of a thousand.
- **Orphaned leftovers**: broken shortcuts, empty folders, abandoned
  application data and installer packages no product references any more.
- **Reports** as a printable page or a spreadsheet, written before anything
  is deleted.

## Download

**[Latest release →](../../releases/latest)**

| File | For |
| --- | --- |
| `DuplicateFinder-*-setup.exe` | Most people. Installs for the current user, no administrator rights needed. |
| `DuplicateFinder-*-portable.exe` | Running it without installing. Put it anywhere. |
| `DuplicateFinder-*.msi` | Deploying it centrally. |

Windows 10 version 1803 or newer, 64-bit. WebView2 ships with Windows 11 and
current Windows 10; the installer fetches it if it is missing.

### The downloads are not code-signed — Windows will warn you

Expect a blue **"Windows protected your PC"** box from SmartScreen. To
continue, click **More info**, then **Run anyway**.

That warning is not a false alarm, and it is worth understanding rather than
clicking past:

- **What it actually says.** The file carries no certificate, so Windows cannot
  tell you who published it or confirm that it has not been altered since it
  was built. SmartScreen warns about every unknown publisher — this is the
  normal experience for a small open-source project, not a sign that something
  is wrong, but it is also not a sign that something is *right*.
- **Why it is unsigned.** A code-signing certificate costs a few hundred euro
  a year and requires identity verification. This is a free project and there
  is nobody to bill it to.
- **Reputation takes time.** Even a signed binary from a new publisher gets
  warned about until SmartScreen has seen enough downloads. A fresh signature
  would not make the warning disappear.

What you can do instead of taking it on faith:

1. **Check the download against `SHA256SUMS.txt`** on the release page:

   ```powershell
   Get-FileHash .\DuplicateFinder-0.1.0-setup.exe -Algorithm SHA256
   ```

   This catches a corrupted or swapped download. It is not a signature: it
   proves the file matches what the release page says, nothing more.

2. **Check where it came from.** Every release is built by the
   [release workflow](.github/workflows/release.yml) on GitHub's runners
   straight from the tagged source, not uploaded from anyone's laptop. The
   build log is public.

3. **Build it yourself** — see [Development](#development). Once the toolchain
   is installed it is one command, and then you are trusting your own compiler
   instead of someone else's upload.

If none of that is enough for you, don't run it. That is a reasonable
position for any unsigned binary, including this one.

### What it does to your machine

Worth knowing before you grant it access to your files:

- It **reads** the folders you point it at. Nothing is sent anywhere — there is
  no network code in the app beyond the installer's WebView2 fetch.
- It **deletes only what you select and confirm**, to the recycle bin unless
  you explicitly choose otherwise. See [Safety](#safety).
- It keeps a **hash cache** in your local app data so a rescan is fast.
- It can ask Windows for a **system restore point** before removing orphans.

## Layout

```
crates/dupe-core/     the engine — no UI dependency, testable on its own
src-tauri/            the Tauri shell: commands, tray, thumbnail protocol
src/                  the Svelte front end
```

The engine is deliberately separate from the app. It can be driven from a
benchmark, a CLI or a test without a webview in sight:

```
cargo run --release -p dupe-core --example probe -- scan    "C:\Users\me\Pictures"
cargo run --release -p dupe-core --example probe -- orphans "C:\Users\me\Desktop"
```

## How the duplicate scan is fast

Four stages, each one only seeing what the previous could not rule out. Most
files are eliminated without a byte of their content being read.

| Stage | What it does | What it costs |
| --- | --- | --- |
| Enumerate | Parallel directory walk, or the NTFS master file table | metadata only |
| Group by size | A unique length means no duplicate | nothing — decided from metadata |
| Partial hash | XXH3 over the first and last 4 KiB plus the size | two seeks |
| Full hash | BLAKE3 over the whole file, memory-mapped | one sequential read |

Extra work is avoided rather than parallelised harder:

- **Hard links are resolved first.** Two paths to the same physical file are
  not duplicates: deleting one frees nothing and deleting both loses the data.
  One path per file id survives into the hashing stages.
- **Small files skip the partial stage.** If the head-and-tail read would have
  covered the whole file anyway, it goes straight to the full hash instead of
  being read twice.
- **Results are cached.** Hashes are keyed on `(file id, size, mtime)` in an
  embedded database, so a rescan reads nothing that has not changed. An edited
  file misses the cache and is re-hashed.
- **The NTFS fast path.** Given whole drives and an elevated process, the
  engine reads the master file table directly instead of walking directories —
  millions of files in seconds. It falls back to the walker automatically when
  that is not available, since it is an optimisation and never a requirement.

BLAKE3 collisions are not a practical risk, so byte-for-byte verification is
available but off by default.

### Cloud placeholders

Files that OneDrive or Dropbox keep online-only are skipped by default, and
reported as skipped rather than silently dropped. Comparing a file means
reading it, and reading a placeholder makes Windows download the whole thing —
so without this guard, scanning a cloud folder could quietly pull down hundreds
of gigabytes and fill the disk. The check is on the `OFFLINE`,
`RECALL_ON_OPEN` and `RECALL_ON_DATA_ACCESS` attributes, and it can be turned
off in the UI.

## How look-alike photos work

Perceptual hashing, two hashes per image, because each fails differently:

- **dHash** compares horizontally adjacent pixels. Immune to brightness and
  gamma shifts — the usual damage from re-encoding — but blind to flat images
  such as document scans and screenshots.
- **pHash** keeps the low-frequency DCT structure. Far more discriminating on
  flat images and survives moderate crops, but reacts to a global tone curve.

Both have to agree before two photos are joined, which is what keeps a photo
library from collapsing into one enormous bogus cluster. Matches are found
through a BK-tree (Hamming distance obeys the triangle inequality, so most of
the tree is never visited) and clustered with a union-find, so a chain of
re-saves ends up in one group even when its ends are far apart.

Images smaller than 64 pixels on their short side are left out: an 8×8 hash
describes the grid rather than the picture at that size, so every emoji and
toolbar sprite matches every other one and buries the actual photos.

Clustering runs in parallel — the tree queries are independent, and the EXIF
capture dates are read up front rather than one file at a time inside the
grouping loop. On a 2,851-photo library that took clustering from 661 ms to
113 ms.

### HEIC, iPhone photos and the embedded thumbnail

An iPhone HEIC is HEVC-coded, and decoding a 12 MP frame costs tens of
milliseconds. But every photo a phone writes already carries a small
pre-rendered preview — the `thmb` item in HEIF, the EXIF thumbnail in JPEG, the
embedded preview in a raw file — and for a 32×32 perceptual hash that preview
carries exactly the same information.

So the decoder asks for the thumbnail first and only falls back to a full
decode when there is none, when it is too small for what the caller needs, or
when its aspect ratio says it has been cropped (which would move the hash).

Decoding goes through the **Windows Imaging Component**, not a bundled decoder.
That keeps HEVC decoding inside the OS component the user is already licensed
for, which answers both the patent question and the licensing question — the
pure-Rust `heic` crate is AGPL-3.0, and `libheif` pulls in libde265. It also
means camera raw and AVIF work wherever the user has those codecs installed.
The portable `image` crate handles whatever WIC declines.

> HEIC support needs the free *HEIF Image Extensions* from the Microsoft Store
> (plus an HEVC codec, usually preinstalled by the OEM). Without them HEIC
> files are reported as skipped rather than silently ignored.

## Orphans

Unlike a hash, "orphaned" is a claim about the rest of the system, so every
finding carries a confidence and a reason you can check. Nothing below
`Verified` is ever preselected.

| Detector | Confidence | Basis |
| --- | --- | --- |
| Broken shortcuts | Verified | The `.lnk` is parsed and its target stat'd |
| Empty folders | Verified | Transitively empty chains, reported at the top |
| Orphaned installer packages | Likely | `.msi`/`.msp` in the installer cache that no product references |
| Leftover application data | Suspected | A name match against the uninstall registry |

Two failure modes get explicit guards:

- The installer detector **refuses to report anything** if it cannot read the
  registry, if the reference list is implausibly short, or if the references it
  found do not line up with the cache. Otherwise "could not look" would be
  indistinguishable from "nothing is referenced", and acting on that would
  break repair and uninstall for every installed product.
- The app-data detector aborts if the installed-product list comes back too
  short, skips folders anything has written to recently, and keeps a hard
  allowlist of Windows-owned folders.

None of these can know a total in advance — the work is "look at everything and
judge it" — so instead of a fabricated percentage they report which detector is
running, how many items it has looked at, and the path it is on, throttled to
about ten updates a second.

## Keeping the window responsive

The engine's threads are not the thing that makes a desktop app feel slow. The
webview is a separate Chromium process with a single main thread for scripting,
layout and paint, and everything the results view does lands on it:

- **Previews are JPEG, not PNG.** A photo compressed losslessly runs about
  70 KB and has to be inflated by that same main thread. JPEG at preview size
  is roughly a tenth of the bytes and far cheaper to decode.
- **Previews are served newest-first.** A first-in-first-out queue spends a
  fast scroll decoding clusters the user has already passed while the ones now
  on screen wait behind them, so the faster you scroll the longer you stare at
  empty frames. The pool is a stack instead. Nothing is dropped, just reordered.
- **Only about three screens of results are in the DOM.** Each group watches
  itself with an `IntersectionObserver` and, when it leaves that band, replaces
  its content with a spacer of the height it last measured — so the scrollbar
  does not move. On a scan with 3,500 clusters this cut the mounted tiles from
  1,705 to 360.
- **The next page loads before the user reaches the bottom**, from a sentinel
  rather than a button.
- **Each tab keeps its own scroll position**, so opening the photos does not
  drop you at whatever offset the duplicate list was left at.
- **Results arrive before the scan ends.** A duplicate is confirmed the moment
  a second file lands on a hash, so the count and the reclaimable total are
  reported while hashing continues. And because the two detectors are
  independent, the duplicates are handed to the UI as soon as they are final
  rather than waiting for the much slower image half — on a cold scan of 53,000
  files that is a browsable list after 55 seconds instead of 2m18s.

Deliberately *not* done: fading previews in as they arrive. It looks better,
but the obvious implementations hide the image until a `load` event that has
often already fired, and the failure mode is an invisible photo.

## Safety

Everything destructive is opt-in, reviewable and, by default, reversible.

- **Nothing is ever deleted automatically.** You select, you confirm.
- **The recycle bin is the default action.** Permanent deletion is a separate
  choice with its own confirmation.
- **A group always keeps one copy.** The UI refuses the click that would select
  the last remaining member, rather than discovering the problem at apply time.
- **`C:\Windows` and `Program Files` are hard-protected**, independently of the
  scan filters — turning off "skip system paths" lets you *see* what is there,
  never delete it. The single exception is narrow and explicit: `.msi`/`.msp`
  files directly inside the installer cache, and only those.
- **Plans are validated as a whole before anything runs.** A plan with any
  refusal does not partially execute.
- **Hard-linking never deletes first.** The original is renamed aside, the link
  is created, and only then is the backup removed — so an interruption leaves
  the data under one name or the other, never neither.

### System restore points

Before removing orphans — and only orphans — the app can ask Windows for a
system restore point.

The scoping is the whole point. A restore point snapshots system files,
installed programs and the registry; it explicitly does not monitor the file
types people keep their own data in. Rolling back will not bring a deleted
photo back. Offering the same option beside the duplicate or photo cleanup
would be a promise the feature cannot keep, and false comfort about deleting
files is worse than none — the recycle bin is the real undo there.

What it *does* cover is exactly what the orphan detectors touch: removing a
package from the installer cache or a folder of leftover application data can
break a product's repair and uninstall path, and that is recoverable from a
restore point.

Two consequences, both deliberate:

- **If the restore point cannot be made, nothing is removed.** Not elevated, or
  system protection switched off, and the operation refuses rather than
  quietly proceeding without the net the user asked for.
- **The state is checked before the user commits**, from a registry key that is
  readable unelevated, so the dialog can say "needs administrator rights" up
  front instead of failing at the moment of truth.

The change is bracketed the way Windows wants — one call opens it, one closes
it — and the handle closes itself on drop, so an early return or a panic cannot
leave the system state manager believing a change is still in progress.

## Reports

A scan can be written out as a file to keep, print or send on — before anything
is deleted, which is usually the point.

- **A page** (`.html`): self-contained, no JavaScript and no external files, so
  it survives being emailed or opened in two years. Photo previews are embedded
  as data URLs for the first 200 clusters; the rest are listed by path, which
  keeps a real scan around two megabytes rather than a hundred. It carries a
  print stylesheet: each section starts on a new page, table headers repeat,
  and groups are not split across a page break.
- **A spreadsheet** (`.csv`): one row per file with a `kind` column, so
  duplicates, photos and orphans can be filtered and totalled together. The
  field separator comes from the machine's own list-separator setting rather
  than being assumed — a comma-separated file opens as a single column in a
  Dutch or German Excel. A UTF-8 BOM goes with it so accented folder names
  survive.

## Icon

`src-tauri/icons/` is generated from `src-tauri/icons/source.png` by `tauri icon`, which
produces the Windows `.ico`, the macOS `.icns` (including the retina variants a
Mac build needs) and the PNG set. The source itself is drawn by
[`tools/make_icon.py`](tools/make_icon.py), so the artwork is reproducible
rather than a binary nobody can edit:

```
python tools/make_icon.py src-tauri/icons/source.png
pnpm tauri icon src-tauri/icons/source.png
```

## Development

You need [Rust](https://rustup.rs) 1.82 or newer, [Node](https://nodejs.org) 20
or newer with [pnpm](https://pnpm.io), and the Tauri prerequisites for Windows
(the MSVC build tools and the WebView2 runtime) — see
[Tauri's setup guide](https://tauri.app/start/prerequisites/).

```
pnpm install
pnpm tauri dev        # vite + the app, with hot reload
pnpm tauri build      # NSIS and MSI installers in target/release/bundle/

cargo test -p dupe-core       # engine tests
pnpm check                    # svelte-check
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

`pnpm tauri build` is the only way to get a real production build. A plain
`cargo build --release` produces a binary that still points at the dev server.

The window hides to the tray on close so a long scan keeps running; quitting is
done from the tray menu, which cancels the scan first so the cache is flushed
cleanly.

### Releasing

Pushing a `v*` tag runs [the release workflow](.github/workflows/release.yml):
it runs the tests, builds the installer, the MSI and the portable executable,
writes `SHA256SUMS.txt` and opens a draft release with all of it attached.

```
git tag v0.1.0 && git push origin v0.1.0
```

### Known gaps

- `csp` is `null` in `tauri.conf.json`. The webview only ever loads local
  content, but a real policy would be better.
- Results are paged into the UI 200 groups at a time; selection applies to what
  has been loaded.
- The MFT fast path lists whole volumes, so it is only chosen when every scan
  root is a drive root.
- Scan settings are not remembered between runs.
- Windows only. Eight source files are `cfg(windows)`-gated; a macOS or Linux
  build would need real work, not a recompile.
- The released binaries are unsigned. See [Download](#download).

## Contributing

Issues and pull requests are welcome. CI runs `svelte-check`, `cargo fmt`,
`clippy` with warnings denied, and the engine tests, so running those four
locally first saves a round trip.

Anything that touches deletion should come with a test. The engine's safety
rules — protected paths, keeping one copy per group, validating a plan as a
whole before it runs — are tested as behaviour rather than documented as
intent, and that is on purpose.

## License

MIT — see [LICENSE](LICENSE).

The dependency tree is permissive throughout (MIT, Apache-2.0, BSD, Zlib,
MPL-2.0). HEVC decoding deliberately goes through the Windows Imaging
Component rather than a bundled decoder, which keeps both the patent and the
copyleft question out of this repository — see
[HEIC, iPhone photos and the embedded thumbnail](#heic-iphone-photos-and-the-embedded-thumbnail).
