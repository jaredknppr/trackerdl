# trackerdl

A powerful terminal-based downloader for (unreleased) music trackers.

## Features

- Interactive TUI (Terminal User Interface) for browsing and selecting tracks
- Batch mode for scripted or automated downloads
- Multi-threaded concurrent downloads with configurable parallelism
- Real-time progress tracking with speed and ETA
- Automatic retry with exponential backoff
- Support for 10+ file hosting services
- Format filtering (FLAC, MP3, M4A, WAV, OGG, AAC)
- Resume support for interrupted downloads
- Rate limiting and bandwidth control
- Proxy support (HTTP/HTTPS)
- Dry-run mode for previewing downloads

## Supported File Hosts

- Pixeldrain (`pixeldrain.com`)
- Pillows (`pillows.su`, `pillowcase.su`)
- KrakenFiles (`krakenfiles.com`)
- imgur.gg (`imgur.gg`)
- SoundCloud (`soundcloud.com`)
- Froste (`music.froste.lol`)
- JuiceWRLD API (`juicewrldapi.com`)

## Installation

### From Source

Requires **Rust 1.70+**

```bash
git clone https://github.com/yourusername/trackerdl.git
cd trackerdl
cargo build --release
```

The binary will be located at:

```
target/release/trackerdl
```

### Pre-built Binaries

Download from the **Releases** page for your platform:

- Linux (x86_64)
- macOS (x86_64, ARM64)
- Windows (x86_64)

## Usage

### Basic Usage

```bash
trackerdl <tracker-id-or-url>
```

The tracker can be either:

- A 44-character tracker ID  
  ```
  1safK4FsrrdxRL5PEF_s-GibgVvyOlTpzx73Mbv-gFFw
  ```
- A full Google Sheets URL containing the tracker ID

### TUI Mode (Default)

Launch the interactive interface:

```bash
trackerdl 1safK4FsrrdxRL5PEF_s-GibgVvyOlTpzx73Mbv-gFFw
```

#### TUI Keyboard Controls

**Navigation**
```
Up / k          Move selection up
Down / j        Move selection down
Page Up         Move up 10 items
Page Down       Move down 10 items
Home            Jump to first item
End             Jump to last item
```

**Selection**
```
Space           Toggle selection on current era
a               Select all eras
n               Deselect all eras
```

**Actions**
```
Enter           Start downloading selected eras
d               Toggle details panel
r               Reload tracker data
s               Show statistics summary
?               Show help screen
```

**General**
```
q / Esc         Quit (or cancel during download)
```

### Batch Mode

Download all tracks without the TUI:

```bash
trackerdl --no-tui <tracker-id>
```

### Common Options

```
-o, --output <DIR>       Output directory (default: ./downloads)
-c, --concurrent <N>     Concurrent downloads (1–20, default: 5)
-t, --tab <NAME>         Load a specific tracker tab
-r, --retries <N>        Retry attempts per download (default: 3)
--format <FMT>           Filter by format (flac, mp3, m4a, wav, ogg)
--dry-run                Preview downloads without downloading
--overwrite              Overwrite existing files
--flat                   Disable subfolders
--numbered               Prefix filenames with track numbers
--quiet                  Minimal output
-v, --verbose            Verbose debug output
```

## Examples

```bash
trackerdl -o ~/Music -c 10 <tracker-id>
```