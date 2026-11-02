# SJK Launcher

A small launcher for Jedi Academy that installs, updates and starts
[Sol JK](https://github.com/Sol-Vulpes/SJK) and
[JoF EternalJK](https://github.com/JediofFreedom/JoF_EJK), and downloads JoF content.

- **Games:** each card shows the version you play and the newest release on GitHub.
  Install, update or switch to any listed release, then hit the big **PLAY**. Archives
  are checked against GitHub's SHA-256 before they're extracted into `GameData`.
- **JoF content:** JoF's maps, skins, hilts and packs from JoF Cloud (then jofhub.eu),
  using SJK's own `sjk-jof-content` crate. Files go in `GameData/SJK/jof`, the same
  folder SJK's JoF content card uses; SJK loads them with *Use JoF content* on. Files
  JoF's launcher already put in `GameData/base` are recognised and not fetched again.
- **Sith cloud:** our own content server (`sith-cloud/`, on the VPS). Same manifest
  format and checks; its files go straight into `GameData/base` so SJK and EternalJK
  both load them. Only files the launcher downloaded from it are ever removed. The
  optional SJK extras pack (`unofficial_sjk_extras.pk3`, in the `extras` group) also
  has a switch of its own at the top of the tab. SJK itself has the same server on its
  Sith cloud card (`sith.downloads`), sharing the launcher's record of what it fetched.
- **SJK chat:** *Chat* in the header (or *SJK chat* in the tray) opens SJK's chat in a
  window of its own, using SJK's `sjk-identity` crate. You are the same player as in
  SJK: the launcher reads `GameData/SJK/identity.key`, `cl_hubUrl` and your `name` from
  SJK's settings, and never makes or changes the key (start SJK once if you have
  none). It follows SJK's chat switch and your muted players (`chat-mutes.txt`). It
  reads the chat only while the window is open, and pauses while a game started from
  the launcher runs, since the game reads it then. Messages go out as
  `SJKLauncher/<version>`, which the hub marks `launcher` so SJK shows the launcher's
  icon beside them.
- **PK3s:** every PK3 in `GameData` (each mod folder, SJK's folder and its JoF
  content), searched by name and by the files inside them: type `kyle` and see every
  PK3 with a file matching it. Each shows what it holds (maps, player models, sabers,
  sounds…), its safety scan, and which PK3s of its folder it replaces files of (the
  game uses the copy in the PK3 last by name). Turn PK3s off (renamed to
  `name.pk3.off`, which the games skip, and back with one click) or delete them, one
  or all those ticked. A file JoF content or the Sith cloud put there is unchosen there
  too, so it doesn't come back. The games' own files (Jedi Academy's `assets0-3`,
  SJK's and EternalJK's folders) are listed but stay.
- **Tray and autostart:** closing the window keeps the launcher in the tray, and
  *Start with my computer* runs it hidden at login (`--minimized`). It checks GitHub
  every few hours and can install new versions by itself while the game is closed.
  Only one runs at a time: starting it again brings the running one up (the SJK and
  JoF launchers each have their own, on local ports 47613 and 47614).
- **Game logs and crashes:** what a game started from here prints goes to
  `logs/sjk.log` or `logs/jof.log` in the launcher's settings folder (the run before
  as `*-previous.log`). If the game ends with an error code, at start or later, the
  launcher comes up with the code in plain words, the last lines, and Copy / Open /
  Play again. For Sol JK it also makes sure SJK's team gets a crash report right
  away: it writes one into SJK's `crashes/` folder when the game could not (built
  from SJK's own `running-<pid>.json` marker when it left one, so no duplicate
  later), with chat and server messages left out, and sends what waits there with
  the player's SJK identity. `cl_crashReports 0` or `cl_identity 0` in SJK keeps it
  from sending.
- **Updates itself:** with every check it also looks at its own releases here. A
  newer one shows *Launcher x.y.z: update and restart* in the header; with *Install
  new versions by themselves* on, it does that alone once no game runs and nothing
  installs. The new program is checked against GitHub's SHA-256, swapped in for the
  running one (which becomes `<name>.old`, deleted at the next start) and started;
  if anything fails the running launcher stays as it was. A launcher built from
  source (debug, or run from a cargo `target*` folder) never updates itself.
- **Developer mode** (Settings, for SJK's developers): builds Sol JK from a source
  checkout and plays that build, like a `play` script. The source folder is found by
  itself (`SJK_SOURCE`, beside the launcher's own source, then the usual clone places
  such as `~/Git/SJK` or `X:\Git\SJK`) or picked by hand. Sol JK's page then shows
  *Developer build*: pick a branch (local ones, then `origin`'s; *Fetch* updates them)
  and **Build and play** runs `cargo build --release -p sjk-viewer -p sjk-dedicated`
  and starts the client from that folder with `GameData` set. The checked-out branch
  builds in place; any other builds in `<source>-wt/test`, checked out detached and
  forced. Each folder keeps its own `target`. Cargo's output goes to
  `logs/sjk-build.log`. A developer's build that goes down gets the crash window but
  no launcher crash report (SJK sends its own, with the right build). Needs git and
  Rust on `PATH`.

`GameData` is found in Steam's libraries, or picked with **Change…**. Settings live in
the platform config folder (`%APPDATA%\SJK\Launcher\config\launcher.json` on Windows).

## The JoF Launcher

The same launcher for the Jedi of Freedom clan, built with `--features jof`
(`src/brand.rs`): the clan's green J o F (drawn from its shapes by
`python scripts/jof_art.py assets/jof`), a green look, JoF EternalJK first, and its own
settings folder (`%APPDATA%\SJK\JoF Launcher`), tray entry and autostart, so both can be
installed side by side. It packs as `jof-launcher-<os>-<arch>`.

## Build

```
./build.ps1          # Windows: build and pack both launchers into dist/
sh build.sh          # Linux and macOS
```

Add `-Test` / `--test` to run the tests first. If Windows refuses to run the script, use
`powershell -ExecutionPolicy Bypass -File build.ps1`.

To release, set the version in `Cargo.toml`, then push a tag `v<version>` (or use
*Run workflow* on the Build action and type the version). The action tests and builds on
all three OSes and publishes a GitHub release with the archives; a version with a `-`,
like `0.2.0-beta.1`, becomes a pre-release.

Linux needs `libgtk-3-dev libxdo-dev libayatana-appindicator3-dev` for the tray.

`cargo test -- --ignored` also asks GitHub for both games' releases and installs both
into a temporary folder.

## Sith cloud server

`sith-cloud` serves a folder as a JoF-style repository on port 8080:

```
C:\ProgramData\SithCloud\
  sith-cloud.exe
  groups.json           names, descriptions and types (REQUIRED/RECOMMENDED/OPTIONAL) per folder
  repository\maps\      one folder per group, PK3s inside
  repository\skins\
  repository\extras\    unofficial_sjk_extras.pk3 (OPTIONAL)
```

Upload a big PK3 under another name outside `repository` first and move it in once
it's whole, so the server never lists a half-copied file.

Add or replace PK3s with scp; the server picks them up within 10 seconds:

```
scp mymap.pk3 sith-cloud:C:/ProgramData/SithCloud/repository/maps/
```

`GET /manifest.json` lists every PK3 with its size and MD5 (cached in
`.sith-cloud-md5.json`); `GET /repository/<group>/<file>` serves it, with `Range` for
resumed downloads. Build it with `cargo build --release -p sith-cloud` (the Windows exe
links the C runtime statically, see `.cargo/config.toml`).