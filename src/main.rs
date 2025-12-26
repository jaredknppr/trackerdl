use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use indexmap::IndexMap;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use regex::Regex;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::{
    io,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

const API_BASE: &str = "https://tracker.israeli.ovh";
const QOBUZ_API: &str = "https://qobuz.squid.wtf/api/download-music";
const VERSION: &str = env!("CARGO_PKG_VERSION");

static TIDAL_APIS: &[&str] = &[
    "https://triton.squid.wtf",
    "https://tidal.kinoplus.online",
    "https://tidal-api.binimum.org",
];

#[derive(Parser, Debug, Clone)]
#[command(name = "trackerdl")]
#[command(version = VERSION)]
#[command(author = "ArtistGrid")]
#[command(about = "Download tracks from ArtistGrid trackers")]
#[command(long_about = "trackerdl - A powerful terminal-based downloader for ArtistGrid music trackers.

DESCRIPTION:
    trackerdl provides both an interactive TUI (Terminal User Interface) and 
    a batch mode for downloading music from ArtistGrid tracker spreadsheets.
    
    It supports multiple file hosts including Pixeldrain, KrakenFiles, 
    imgur.gg, Tidal, Qobuz, SoundCloud, and more.

EXAMPLES:
    Interactive TUI mode:
        trackerdl 1safK4FsrrdxRL5PEF_s-GibgVvyOlTpzx73Mbv-gFFw
        
    Download all tracks (batch mode):
        trackerdl --no-tui <tracker-id>
        
    Download to specific folder with 10 concurrent downloads:
        trackerdl -o ~/Music -c 10 <tracker-id>
        
    Download only FLAC files:
        trackerdl --format flac <tracker-id>
        
    Preview what would be downloaded:
        trackerdl --dry-run <tracker-id>
        
    Load specific tab from tracker:
        trackerdl --tab \"Studio Albums\" <tracker-id>

TUI CONTROLS:
    Press '?' in the TUI for full keyboard controls.
    
    Quick reference:
        Up/Down or j/k    Navigate eras
        Space             Toggle selection
        a/n               Select all / none
        Enter             Start download
        q/Esc             Quit

For more information, visit: https://github.com/artistgrid/trackerdl")]
struct Cli {
    #[arg(help = "Google Sheet URL or tracker ID (44 characters)")]
    tracker: String,

    #[arg(short, long, default_value = "./downloads", help = "Output directory for downloaded files")]
    output: PathBuf,

    #[arg(short, long, default_value = "5", help = "Number of concurrent downloads (1-20)")]
    concurrent: usize,

    #[arg(short, long, help = "Specific tab name to load from tracker")]
    tab: Option<String>,

    #[arg(long, help = "Skip TUI and download all tracks automatically (batch mode)")]
    no_tui: bool,

    #[arg(long, help = "Override artist name for folder structure")]
    artist: Option<String>,

    #[arg(short, long, default_value = "3", help = "Number of retry attempts per failed download (uses exponential backoff)")]
    retries: usize,

    #[arg(long, default_value = "300", help = "Request timeout in seconds for each download")]
    timeout: u64,

    #[arg(short, long, help = "Enable verbose output with debug information")]
    verbose: bool,

    #[arg(long, help = "Show what would be downloaded without actually downloading")]
    dry_run: bool,

    #[arg(long, help = "Skip files that already exist (this is the default behavior)")]
    skip_existing: bool,

    #[arg(long, help = "Overwrite existing files instead of skipping them")]
    overwrite: bool,

    #[arg(long, help = "Filter by file format (flac, mp3, m4a, wav, ogg, aac)")]
    format: Option<String>,

    #[arg(long, help = "Filter eras by name (case-insensitive substring match)")]
    filter_era: Option<String>,

    #[arg(long, help = "Minimum file size in MB to download")]
    min_size: Option<f64>,

    #[arg(long, help = "Maximum file size in MB to download")]
    max_size: Option<f64>,

    #[arg(long, help = "Export track list to JSON file instead of downloading")]
    export_json: Option<PathBuf>,

    #[arg(long, help = "Export track list to CSV file instead of downloading")]
    export_csv: Option<PathBuf>,

    #[arg(long, help = "Use flat directory structure (no era/album subfolders)")]
    flat: bool,

    #[arg(long, help = "Add track number prefix to filenames (001 - Song.mp3)")]
    numbered: bool,

    #[arg(long, help = "Custom user agent string for HTTP requests")]
    user_agent: Option<String>,

    #[arg(long, help = "HTTP/HTTPS proxy URL (e.g., http://127.0.0.1:8080)")]
    proxy: Option<String>,

    #[arg(long, help = "Disable SSL certificate verification (INSECURE - use with caution)")]
    insecure: bool,

    #[arg(long, help = "Rate limit in KB/s per download (0 = unlimited)")]
    rate_limit: Option<u64>,

    #[arg(long, help = "Delay between starting downloads in milliseconds")]
    delay: Option<u64>,

    #[arg(long, help = "Only show tracker statistics without downloading anything")]
    stats_only: bool,

    #[arg(long, help = "Continue incomplete downloads (resume support where available)")]
    resume: bool,

    #[arg(long, help = "Verify file integrity after download using Content-Length")]
    verify: bool,

    #[arg(long, help = "Log all output to specified file")]
    log_file: Option<PathBuf>,

    #[arg(long, help = "Quiet mode - only show errors and final summary")]
    quiet: bool,

    #[arg(long, help = "Show list of supported file hosts and exit")]
    list_hosts: bool,

    #[arg(long, help = "Test connectivity to all backend APIs and exit")]
    test_apis: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct TrackerResponse {
    name: Option<String>,
    tabs: Vec<String>,
    current_tab: String,
    #[serde(deserialize_with = "deserialize_eras")]
    eras: IndexMap<String, Era>,
}

fn deserialize_eras<'de, D>(deserializer: D) -> Result<IndexMap<String, Era>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    IndexMap::deserialize(deserializer)
}

#[derive(Debug, Clone, Deserialize)]
struct Era {
    name: String,
    #[serde(default)]
    data: Option<IndexMap<String, Vec<Track>>>,
}

#[derive(Debug, Clone, Deserialize)]
struct Track {
    name: String,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    available_length: Option<String>,
}

#[derive(Debug, Clone)]
struct EraItem {
    key: String,
    name: String,
    track_count: usize,
    selected: bool,
    order: usize,
    categories: Vec<String>,
}

#[derive(Debug, Clone)]
struct DownloadTrack {
    id: usize,
    name: String,
    era_name: String,
    original_url: String,
    playable_url: String,
    host: String,
}

#[derive(Debug, Clone)]
struct ActiveDownload {
    id: usize,
    name: String,
    era_name: String,
    host: String,
    url: String,
    downloaded: u64,
    total: Option<u64>,
    status: DownloadStatus,
    started_at: Option<Instant>,
    speed_samples: Vec<(Instant, u64)>,
    retries: usize,
    error: Option<String>,
}

impl ActiveDownload {
    fn current_speed(&self) -> f64 {
        if self.speed_samples.len() < 2 {
            return 0.0;
        }
        let recent: Vec<_> = self.speed_samples.iter().rev().take(10).collect();
        if recent.len() < 2 {
            return 0.0;
        }
        let (newest_time, newest_bytes) = recent.first().unwrap();
        let (oldest_time, oldest_bytes) = recent.last().unwrap();
        let duration = newest_time.duration_since(*oldest_time).as_secs_f64();
        if duration > 0.0 {
            (*newest_bytes - *oldest_bytes) as f64 / duration
        } else {
            0.0
        }
    }

    fn eta(&self) -> Option<Duration> {
        let speed = self.current_speed();
        if speed <= 0.0 {
            return None;
        }
        let remaining = self.total.unwrap_or(0).saturating_sub(self.downloaded);
        Some(Duration::from_secs_f64(remaining as f64 / speed))
    }

    fn elapsed(&self) -> Option<Duration> {
        self.started_at.map(|s| s.elapsed())
    }
}

#[derive(Debug, Clone, PartialEq)]
enum DownloadStatus {
    Pending,
    Resolving,
    Downloading,
    Complete,
    Failed,
    Skipped,
    Retrying,
}

#[derive(Debug, Clone, Default)]
struct DownloadState {
    downloads: Vec<ActiveDownload>,
    completed: usize,
    failed: usize,
    skipped: usize,
    total: usize,
    total_bytes_downloaded: u64,
    started_at: Option<Instant>,
}

impl DownloadState {
    fn active_downloads(&self) -> Vec<&ActiveDownload> {
        self.downloads
            .iter()
            .filter(|d| {
                d.status == DownloadStatus::Downloading
                    || d.status == DownloadStatus::Resolving
                    || d.status == DownloadStatus::Retrying
            })
            .collect()
    }

    fn progress_percent(&self) -> u16 {
        if self.total == 0 {
            0
        } else {
            ((self.completed + self.failed + self.skipped) as f64 / self.total as f64 * 100.0)
                as u16
        }
    }

    fn overall_speed(&self) -> f64 {
        if let Some(started) = self.started_at {
            let elapsed = started.elapsed().as_secs_f64();
            if elapsed > 0.0 {
                return self.total_bytes_downloaded as f64 / elapsed;
            }
        }
        0.0
    }

    fn elapsed(&self) -> Option<Duration> {
        self.started_at.map(|s| s.elapsed())
    }

    fn hosts_summary(&self) -> IndexMap<String, (usize, usize, usize)> {
        let mut summary: IndexMap<String, (usize, usize, usize)> = IndexMap::new();
        for dl in &self.downloads {
            let entry = summary.entry(dl.host.clone()).or_insert((0, 0, 0));
            match dl.status {
                DownloadStatus::Complete => entry.0 += 1,
                DownloadStatus::Failed => entry.1 += 1,
                DownloadStatus::Skipped => entry.2 += 1,
                _ => {}
            }
        }
        summary
    }
}

#[derive(Debug, Clone)]
struct TrackerStats {
    total_eras: usize,
    total_tracks: usize,
    tracks_with_urls: usize,
    tracks_without_urls: usize,
    hosts: IndexMap<String, usize>,
    formats: IndexMap<String, usize>,
}

struct App {
    cli: Cli,
    client: Client,
    tracker_id: String,
    tracker_data: Option<TrackerResponse>,
    eras: Vec<EraItem>,
    list_state: ListState,
    status: String,
    downloading: bool,
    download_state: Arc<RwLock<DownloadState>>,
    should_quit: bool,
    stats: Option<TrackerStats>,
    show_details: bool,
    show_help: bool,
    log_messages: Vec<(Instant, String)>,
}

impl App {
    async fn new(cli: Cli) -> Result<Self> {
        let tracker_id = extract_tracker_id(&cli.tracker)?;

        let user_agent = cli.user_agent.clone().unwrap_or_else(|| {
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string()
        });

        let mut client_builder = Client::builder()
            .timeout(Duration::from_secs(cli.timeout))
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(10)
            .tcp_keepalive(Duration::from_secs(60));

        if cli.insecure {
            client_builder = client_builder.danger_accept_invalid_certs(true);
        }

        if let Some(ref proxy_url) = cli.proxy {
            let proxy = reqwest::Proxy::all(proxy_url)?;
            client_builder = client_builder.proxy(proxy);
        }

        let client = client_builder.build()?;

        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Ok(Self {
            cli,
            client,
            tracker_id,
            tracker_data: None,
            eras: Vec::new(),
            list_state,
            status: String::new(),
            downloading: false,
            download_state: Arc::new(RwLock::new(DownloadState::default())),
            should_quit: false,
            stats: None,
            show_details: false,
            show_help: false,
            log_messages: Vec::new(),
        })
    }

    fn log(&mut self, msg: String) {
        if self.cli.verbose {
            self.log_messages.push((Instant::now(), msg));
            if self.log_messages.len() > 100 {
                self.log_messages.remove(0);
            }
        }
    }

    async fn load_tracker(&mut self) -> Result<()> {
        self.status = "Loading tracker...".to_string();
        self.log(format!("Fetching tracker ID: {}", self.tracker_id));

        let url = match &self.cli.tab {
            Some(t) => format!("{}/get/{}?tab={}", API_BASE, self.tracker_id, t),
            None => format!("{}/get/{}", API_BASE, self.tracker_id),
        };

        self.log(format!("API URL: {}", url));

        let start = Instant::now();
        let response = self.client.get(&url).send().await?;
        let elapsed = start.elapsed();

        self.log(format!("Response: {} in {:?}", response.status(), elapsed));

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to load tracker: HTTP {}",
                response.status()
            ));
        }

        let data: TrackerResponse = response.json().await?;

        let mut total_tracks = 0;
        let mut tracks_with_urls = 0;
        let mut tracks_without_urls = 0;
        let mut hosts: IndexMap<String, usize> = IndexMap::new();
        let mut formats: IndexMap<String, usize> = IndexMap::new();

        self.eras = data
            .eras
            .iter()
            .enumerate()
            .filter(|(_, (_, era))| {
                if let Some(ref filter) = self.cli.filter_era {
                    era.name.to_lowercase().contains(&filter.to_lowercase())
                } else {
                    true
                }
            })
            .map(|(order, (key, era))| {
                let mut track_count = 0;
                let mut categories = Vec::new();

                if let Some(ref data) = era.data {
                    for (cat_name, tracks) in data {
                        categories.push(cat_name.clone());
                        for track in tracks {
                            track_count += 1;
                            total_tracks += 1;

                            if let Some(url) = get_track_url(track) {
                                tracks_with_urls += 1;
                                let host = detect_host(&url);
                                *hosts.entry(host).or_insert(0) += 1;

                                let format = detect_format(&url);
                                *formats.entry(format).or_insert(0) += 1;
                            } else {
                                tracks_without_urls += 1;
                            }
                        }
                    }
                }

                EraItem {
                    key: key.clone(),
                    name: era.name.clone(),
                    track_count,
                    selected: false,
                    order,
                    categories,
                }
            })
            .collect();

        self.eras.sort_by_key(|e| e.order);

        self.stats = Some(TrackerStats {
            total_eras: self.eras.len(),
            total_tracks,
            tracks_with_urls,
            tracks_without_urls,
            hosts,
            formats,
        });

        self.tracker_data = Some(data);
        self.status = format!(
            "Loaded {} eras, {} tracks ({} with URLs)",
            self.eras.len(),
            total_tracks,
            tracks_with_urls
        );

        Ok(())
    }

    fn toggle_selection(&mut self) {
        if let Some(i) = self.list_state.selected() {
            if i < self.eras.len() {
                self.eras[i].selected = !self.eras[i].selected;
            }
        }
    }

    fn select_all(&mut self) {
        for era in &mut self.eras {
            era.selected = true;
        }
    }

    fn deselect_all(&mut self) {
        for era in &mut self.eras {
            era.selected = false;
        }
    }

    fn selected_count(&self) -> usize {
        self.eras.iter().filter(|e| e.selected).count()
    }

    fn selected_track_count(&self) -> usize {
        self.eras
            .iter()
            .filter(|e| e.selected)
            .map(|e| e.track_count)
            .sum()
    }

    async fn download_selected(&mut self) -> Result<()> {
        let data = self
            .tracker_data
            .as_ref()
            .ok_or_else(|| anyhow!("No data"))?;

        let selected_keys: Vec<String> = self
            .eras
            .iter()
            .filter(|e| e.selected)
            .map(|e| e.key.clone())
            .collect();

        if selected_keys.is_empty() {
            self.status = "Nothing selected".to_string();
            return Ok(());
        }

        self.downloading = true;
        self.status = "Resolving track URLs...".to_string();

        let mut tracks_to_download: Vec<DownloadTrack> = Vec::new();
        let mut id_counter = 0;

        for key in &selected_keys {
            if let Some(era) = data.eras.get(key) {
                if let Some(ref categories) = era.data {
                    for track_list in categories.values() {
                        for track in track_list {
                            if let Some(url) = get_track_url(track) {
                                if let Some(ref format_filter) = self.cli.format {
                                    let detected = detect_format(&url);
                                    if !detected.eq_ignore_ascii_case(format_filter) {
                                        continue;
                                    }
                                }

                                let host = detect_host(&url);

                                if let Some(playable) =
                                    resolve_playable_url(&self.client, &url).await
                                {
                                    tracks_to_download.push(DownloadTrack {
                                        id: id_counter,
                                        name: track.name.clone(),
                                        era_name: era.name.clone(),
                                        original_url: url.clone(),
                                        playable_url: playable,
                                        host,
                                    });
                                    id_counter += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        let total = tracks_to_download.len();
        if total == 0 {
            self.status = "No playable tracks found".to_string();
            self.downloading = false;
            return Ok(());
        }

        if self.cli.dry_run {
            self.status = format!("Dry run: would download {} tracks", total);
            self.downloading = false;
            for track in &tracks_to_download {
                println!(
                    "[DRY RUN] {} - {} ({})",
                    track.era_name, track.name, track.host
                );
                println!("          URL: {}", track.playable_url);
            }
            return Ok(());
        }

        {
            let mut state = self.download_state.write().unwrap();
            state.downloads = tracks_to_download
                .iter()
                .map(|t| ActiveDownload {
                    id: t.id,
                    name: t.name.clone(),
                    era_name: t.era_name.clone(),
                    host: t.host.clone(),
                    url: t.playable_url.clone(),
                    downloaded: 0,
                    total: None,
                    status: DownloadStatus::Pending,
                    started_at: None,
                    speed_samples: Vec::new(),
                    retries: 0,
                    error: None,
                })
                .collect();
            state.total = total;
            state.completed = 0;
            state.failed = 0;
            state.skipped = 0;
            state.total_bytes_downloaded = 0;
            state.started_at = Some(Instant::now());
        }

        self.status = format!("Downloading {} tracks...", total);

        let artist_name = self
            .cli
            .artist
            .clone()
            .or_else(|| self.tracker_data.as_ref().and_then(|d| d.name.clone()))
            .unwrap_or_else(|| "Unknown Artist".to_string());

        let base_dir = self
            .cli
            .output
            .join(sanitize_filename::sanitize(&artist_name));
        std::fs::create_dir_all(&base_dir)?;

        let client = self.client.clone();
        let concurrent = self.cli.concurrent.clamp(1, 20);
        let download_state = self.download_state.clone();
        let cli = self.cli.clone();

        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrent));
        let mut handles = Vec::new();

        for (index, track) in tracks_to_download.into_iter().enumerate() {
            let client = client.clone();
            let base_dir = base_dir.clone();
            let download_state = download_state.clone();
            let cli = cli.clone();
            let permit = semaphore.clone().acquire_owned().await.unwrap();

            let handle = tokio::spawn(async move {
                if let Some(delay_ms) = cli.delay {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }

                let mut attempts = 0;
                let max_retries = cli.retries;
                let mut last_error: Option<String> = None;

                loop {
                    let result = download_track_with_progress(
                        &client,
                        &track,
                        &base_dir,
                        download_state.clone(),
                        &cli,
                        index,
                    )
                    .await;

                    match result {
                        Ok(success) => {
                            let mut state = download_state.write().unwrap();
                            if let Some(dl) =
                                state.downloads.iter_mut().find(|d| d.id == track.id)
                            {
                                if success {
                                    dl.status = DownloadStatus::Complete;
                                    state.completed += 1;
                                } else {
                                    dl.status = DownloadStatus::Skipped;
                                    state.skipped += 1;
                                }
                            }
                            break;
                        }
                        Err(e) => {
                            attempts += 1;
                            last_error = Some(e.to_string());

                            if attempts > max_retries {
                                let mut state = download_state.write().unwrap();
                                if let Some(dl) =
                                    state.downloads.iter_mut().find(|d| d.id == track.id)
                                {
                                    dl.status = DownloadStatus::Failed;
                                    dl.error = last_error.clone();
                                    state.failed += 1;
                                }
                                break;
                            } else {
                                {
                                    let mut state = download_state.write().unwrap();
                                    if let Some(dl) =
                                        state.downloads.iter_mut().find(|d| d.id == track.id)
                                    {
                                        dl.status = DownloadStatus::Retrying;
                                        dl.retries = attempts;
                                    }
                                }
                                tokio::time::sleep(Duration::from_secs(2_u64.pow(attempts as u32)))
                                    .await;
                            }
                        }
                    }
                }

                drop(permit);
            });

            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }

        let state = self.download_state.read().unwrap();
        let elapsed = state
            .elapsed()
            .map(|d| format!("{:.1}s", d.as_secs_f64()))
            .unwrap_or_default();
        let speed = format_bytes_speed(state.overall_speed());

        self.status = format!(
            "Done! {} completed, {} failed, {} skipped | {} | {}",
            state.completed, state.failed, state.skipped, elapsed, speed
        );
        self.downloading = false;

        Ok(())
    }
}

async fn download_track_with_progress(
    client: &Client,
    track: &DownloadTrack,
    base_dir: &PathBuf,
    download_state: Arc<RwLock<DownloadState>>,
    cli: &Cli,
    index: usize,
) -> Result<bool> {
    let era_dir = if cli.flat {
        base_dir.clone()
    } else {
        base_dir.join(sanitize_filename::sanitize(&track.era_name))
    };
    std::fs::create_dir_all(&era_dir)?;

    let ext = get_file_extension(&track.playable_url);
    let filename = if cli.numbered {
        format!(
            "{:03} - {}.{}",
            index + 1,
            sanitize_filename::sanitize(&track.name),
            ext
        )
    } else {
        format!("{}.{}", sanitize_filename::sanitize(&track.name), ext)
    };
    let path = era_dir.join(&filename);

    if path.exists() && !cli.overwrite {
        return Ok(false);
    }

    {
        let mut state = download_state.write().unwrap();
        if let Some(dl) = state.downloads.iter_mut().find(|d| d.id == track.id) {
            dl.status = DownloadStatus::Downloading;
            dl.started_at = Some(Instant::now());
        }
    }

    let response = client.get(&track.playable_url).send().await?;

    if !response.status().is_success() {
        return Err(anyhow!("HTTP {}", response.status()));
    }

    let total_size = response.content_length();

    if let Some(min_mb) = cli.min_size {
        if let Some(size) = total_size {
            if (size as f64 / 1_048_576.0) < min_mb {
                return Ok(false);
            }
        }
    }
    if let Some(max_mb) = cli.max_size {
        if let Some(size) = total_size {
            if (size as f64 / 1_048_576.0) > max_mb {
                return Ok(false);
            }
        }
    }

    {
        let mut state = download_state.write().unwrap();
        if let Some(dl) = state.downloads.iter_mut().find(|d| d.id == track.id) {
            dl.total = total_size;
        }
    }

    let mut downloaded: u64 = 0;
    let mut bytes_vec = Vec::new();
    let mut stream = response.bytes_stream();
    let rate_limit = cli.rate_limit.unwrap_or(0) * 1024;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        let chunk_len = chunk.len() as u64;
        downloaded += chunk_len;
        bytes_vec.extend_from_slice(&chunk);

        {
            let mut state = download_state.write().unwrap();
            state.total_bytes_downloaded += chunk_len;
            if let Some(dl) = state.downloads.iter_mut().find(|d| d.id == track.id) {
                dl.downloaded = downloaded;
                dl.speed_samples.push((Instant::now(), downloaded));
                if dl.speed_samples.len() > 50 {
                    dl.speed_samples.remove(0);
                }
            }
        }

        if rate_limit > 0 {
            let expected_time = downloaded as f64 / rate_limit as f64;
            let actual_time = {
                let state = download_state.read().unwrap();
                state
                    .downloads
                    .iter()
                    .find(|d| d.id == track.id)
                    .and_then(|d| d.started_at)
                    .map(|s| s.elapsed().as_secs_f64())
                    .unwrap_or(0.0)
            };
            if actual_time < expected_time {
                tokio::time::sleep(Duration::from_secs_f64(expected_time - actual_time)).await;
            }
        }
    }

    if cli.verify {
        if let Some(expected) = total_size {
            if bytes_vec.len() as u64 != expected {
                return Err(anyhow!(
                    "Size mismatch: expected {} got {}",
                    expected,
                    bytes_vec.len()
                ));
            }
        }
    }

    std::fs::write(&path, &bytes_vec)?;

    Ok(true)
}

fn extract_tracker_id(input: &str) -> Result<String> {
    let input = input.trim();
    if input.len() == 44
        && input
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Ok(input.to_string());
    }
    let re = Regex::new(r"([a-zA-Z0-9_-]{44})")?;
    if let Some(caps) = re.captures(input) {
        if let Some(id) = caps.get(1) {
            return Ok(id.as_str().to_string());
        }
    }
    Err(anyhow!("Invalid tracker ID or URL"))
}

fn get_track_url(track: &Track) -> Option<String> {
    for field in [&track.url, &track.quality, &track.available_length] {
        if let Some(ref url) = field {
            if url.starts_with("http://") || url.starts_with("https://") {
                return Some(url.replace("pillowcase.su", "pillows.su"));
            }
        }
    }
    None
}

fn detect_host(url: &str) -> String {
    if url.contains("pixeldrain.com") {
        "pixeldrain".to_string()
    } else if url.contains("pillows.su") || url.contains("pillowcase.su") {
        "pillows".to_string()
    } else if url.contains("krakenfiles.com") {
        "krakenfiles".to_string()
    } else if url.contains("imgur.gg") {
        "imgur.gg".to_string()
    } else if url.contains("soundcloud.com") {
        "soundcloud".to_string()
    } else if url.contains("tidal.com") {
        "tidal".to_string()
    } else if url.contains("qobuz.com") {
        "qobuz".to_string()
    } else if url.contains("froste.lol") {
        "froste".to_string()
    } else if url.contains("juicewrldapi.com") {
        "juicewrld".to_string()
    } else {
        "unknown".to_string()
    }
}

fn detect_format(url: &str) -> String {
    let url_lower = url.to_lowercase();
    if url_lower.contains(".flac") {
        "flac".to_string()
    } else if url_lower.contains(".m4a") {
        "m4a".to_string()
    } else if url_lower.contains(".wav") {
        "wav".to_string()
    } else if url_lower.contains(".ogg") {
        "ogg".to_string()
    } else if url_lower.contains(".aac") {
        "aac".to_string()
    } else {
        "mp3".to_string()
    }
}

async fn resolve_playable_url(client: &Client, url: &str) -> Option<String> {
    if let Some(caps) = Regex::new(r"pixeldrain\.com/u/([a-zA-Z0-9]+)")
        .ok()?
        .captures(url)
    {
        return Some(format!(
            "https://pixeldrain.com/api/file/{}",
            caps.get(1)?.as_str()
        ));
    }

    if let Some(caps) = Regex::new(r"pillows\.su/f/([a-f0-9]+)")
        .ok()?
        .captures(url)
    {
        return Some(format!(
            "https://api.pillows.su/api/download/{}",
            caps.get(1)?.as_str()
        ));
    }

    if let Some(caps) = Regex::new(r"music\.froste\.lol/song/([a-f0-9]+)")
        .ok()?
        .captures(url)
    {
        return Some(format!(
            "https://music.froste.lol/song/{}/download",
            caps.get(1)?.as_str()
        ));
    }

    if let Some(caps) = Regex::new(r"krakenfiles\.com/view/([a-zA-Z0-9]+)")
        .ok()?
        .captures(url)
    {
        let id = caps.get(1)?.as_str();
        if let Some(m4a_url) = scrape_krakenfiles(client, id).await {
            return Some(m4a_url);
        }
    }

    if let Some(caps) = Regex::new(r"imgur\.gg/f/([a-zA-Z0-9]+)")
        .ok()?
        .captures(url)
    {
        let id = caps.get(1)?.as_str();
        if let Some(mp3_url) = scrape_imgur(client, id).await {
            return Some(mp3_url);
        }
    }

    if let Some(caps) = Regex::new(r"soundcloud\.com/([^/]+/[^/?#]+)")
        .ok()?
        .captures(url)
    {
        return Some(format!(
            "https://sc.maid.zone/_/restream/{}",
            caps.get(1)?.as_str()
        ));
    }

    if let Some(caps) = Regex::new(r"tidal\.com/(?:browse/)?track/(\d+)")
        .ok()?
        .captures(url)
    {
        let id = caps.get(1)?.as_str();
        for api_base in TIDAL_APIS {
            let api_url = format!("{}/track/?id={}&quality=HI_RES_LOSSLESS", api_base, id);
            if let Ok(resp) = client.get(&api_url).send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(manifest) = json
                        .get("data")
                        .and_then(|d| d.get("manifest"))
                        .and_then(|m| m.as_str())
                    {
                        if let Ok(decoded) = STANDARD.decode(manifest) {
                            if let Ok(mj) = serde_json::from_slice::<serde_json::Value>(&decoded) {
                                if let Some(u) = mj
                                    .get("urls")
                                    .and_then(|u| u.get(0))
                                    .and_then(|u| u.as_str())
                                {
                                    return Some(u.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(caps) = Regex::new(r"qobuz\.com/track/(\d+)")
        .ok()?
        .captures(url)
    {
        let api_url = format!("{}?track_id={}&quality=27", QOBUZ_API, caps.get(1)?.as_str());
        if let Ok(resp) = client.get(&api_url).send().await {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                return json
                    .get("data")
                    .and_then(|d| d.get("url"))
                    .and_then(|u| u.as_str())
                    .map(|s| s.to_string());
            }
        }
    }

    if url.contains("juicewrldapi.com") {
        return Some(url.to_string());
    }

    None
}

async fn scrape_krakenfiles(client: &Client, id: &str) -> Option<String> {
    let url = format!("https://krakenfiles.com/view/{}/file.html", id);

    let resp = client
        .get(&url)
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let html = resp.text().await.ok()?;

    let m4a_regex = Regex::new(r#"m4a:\s*['"]([^'"]+)['"]"#).ok()?;
    if let Some(caps) = m4a_regex.captures(&html) {
        let mut m4a_url = caps.get(1)?.as_str().to_string();
        if m4a_url.starts_with("//") {
            m4a_url = format!("https:{}", m4a_url);
        }
        return Some(m4a_url);
    }

    None
}

async fn scrape_imgur(client: &Client, id: &str) -> Option<String> {
    let url = format!("https://imgur.gg/f/{}", id);

    let resp = client
        .get(&url)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let html = resp.text().await.ok()?;
    let document = Html::parse_document(&html);

    if let Ok(audio_selector) = Selector::parse("audio") {
        if let Some(audio_element) = document.select(&audio_selector).next() {
            if let Some(src) = audio_element.value().attr("src") {
                return Some(src.to_string());
            }
        }
    }

    if let Ok(source_selector) = Selector::parse("audio source") {
        if let Some(source_element) = document.select(&source_selector).next() {
            if let Some(src) = source_element.value().attr("src") {
                return Some(src.to_string());
            }
        }
    }

    if let Ok(script_selector) = Selector::parse("script") {
        for script in document.select(&script_selector) {
            let text = script.text().collect::<String>();
            if text.contains("bucketKey") || text.contains("self.__next_f.push") {
                if let Ok(url_regex) = Regex::new(r#"https://[^"'\s\\]+\.(?:mp3|m4a|flac|wav)"#) {
                    if let Some(caps) = url_regex.captures(&text) {
                        return Some(caps.get(0)?.as_str().to_string());
                    }
                }
            }
        }
    }

    None
}

fn get_file_extension(url: &str) -> &'static str {
    let url_lower = url.to_lowercase();
    if url_lower.contains(".flac") {
        "flac"
    } else if url_lower.contains(".m4a") {
        "m4a"
    } else if url_lower.contains(".ogg") {
        "ogg"
    } else if url_lower.contains(".wav") {
        "wav"
    } else if url_lower.contains(".aac") {
        "aac"
    } else {
        "mp3"
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_bytes_speed(bytes_per_sec: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;

    if bytes_per_sec >= MB {
        format!("{:.2} MB/s", bytes_per_sec / MB)
    } else if bytes_per_sec >= KB {
        format!("{:.2} KB/s", bytes_per_sec / KB)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!(
            "{}h {:02}m {:02}s",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60
        )
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn render_help_overlay(f: &mut Frame) {
    let area = f.size();
    let popup_area = centered_rect(80, 85, area);

    f.render_widget(Clear, popup_area);

    let help_text = r#"
NAVIGATION
────────────────────────────────────────────────────────────────
  ↑ / k              Move selection up
  ↓ / j              Move selection down
  Page Up            Move up 10 items
  Page Down          Move down 10 items
  Home               Jump to first item
  End                Jump to last item

SELECTION
────────────────────────────────────────────────────────────────
  Space              Toggle selection on current era
  a                  Select all eras
  n                  Deselect all eras (none)

ACTIONS
────────────────────────────────────────────────────────────────
  Enter              Start downloading selected eras
  d                  Toggle details panel
  r                  Reload tracker data
  s                  Show statistics summary in status bar

GENERAL
────────────────────────────────────────────────────────────────
  ?                  Show/hide this help screen
  q / Esc            Quit application (or close this help)

DURING DOWNLOAD
────────────────────────────────────────────────────────────────
  Downloads run in background with real-time progress display.
  Press q or Esc to abort all downloads and exit.

COMMAND LINE TIPS
────────────────────────────────────────────────────────────────
  --no-tui           Batch mode (download all without TUI)
  --dry-run          Preview what would be downloaded
  --format flac      Filter by audio format
  --concurrent 10    Set parallel download count
  --help             Show all command line options

                    Press ? or Esc to close this help
"#;

    let help_paragraph = Paragraph::new(help_text)
        .style(Style::default().fg(Color::White).bg(Color::Black))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Help - Keyboard Controls ")
                .title_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .style(Style::default().bg(Color::Black)),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(help_paragraph, popup_area);
}

fn render_ui(f: &mut Frame, app: &App) {
    let area = f.size();

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(area);

    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(14),
            Constraint::Length(3),
        ])
        .split(main_chunks[0]);

    let tracker_name = app
        .tracker_data
        .as_ref()
        .and_then(|d| d.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());
    let current_tab = app
        .tracker_data
        .as_ref()
        .map(|d| d.current_tab.clone())
        .unwrap_or_default();
    let tabs_count = app.tracker_data.as_ref().map(|d| d.tabs.len()).unwrap_or(0);

    let title = format!(
        " trackerdl v{} | {} | Tab: {} ({} available) | ID: {}...{} ",
        VERSION,
        tracker_name,
        current_tab,
        tabs_count,
        &app.tracker_id[..8.min(app.tracker_id.len())],
        &app.tracker_id[app.tracker_id.len().saturating_sub(4)..]
    );
    let header = Paragraph::new(title)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, left_chunks[0]);

    let items: Vec<ListItem> = app
        .eras
        .iter()
        .map(|era| {
            let checkbox = if era.selected { "[x]" } else { "[ ]" };
            let cats = era.categories.len();
            let text = format!(
                "{} {} ({} tracks, {} categories)",
                checkbox, era.name, era.track_count, cats
            );
            let style = if era.selected {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(text).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default().borders(Borders::ALL).title(format!(
                " Eras ({}/{} selected, {} tracks) ",
                app.selected_count(),
                app.eras.len(),
                app.selected_track_count()
            )),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("► ");

    let mut state = app.list_state.clone();
    f.render_stateful_widget(list, left_chunks[1], &mut state);

    if app.downloading {
        let download_state = app.download_state.read().unwrap();
        let pct = download_state.progress_percent();
        let completed = download_state.completed + download_state.failed + download_state.skipped;
        let speed = format_bytes_speed(download_state.overall_speed());
        let elapsed = download_state
            .elapsed()
            .map(format_duration)
            .unwrap_or_default();
        let total_dl = format_bytes(download_state.total_bytes_downloaded);

        let gauge = Gauge::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Overall Progress "),
            )
            .gauge_style(Style::default().fg(Color::Cyan))
            .percent(pct)
            .label(format!(
                "{}/{} | ✓{} ✗{} ⊘{} | {} | {} | {}",
                completed,
                download_state.total,
                download_state.completed,
                download_state.failed,
                download_state.skipped,
                speed,
                total_dl,
                elapsed
            ));
        f.render_widget(gauge, left_chunks[2]);
    } else {
        let help = Paragraph::new(
            " Space: toggle | a: all | n: none | Enter: download | ?: help | q: quit ",
        )
        .style(Style::default().fg(Color::Gray))
        .block(Block::default().borders(Borders::ALL).title(" Controls "));
        f.render_widget(help, left_chunks[2]);
    }

    if app.downloading {
        let download_state = app.download_state.read().unwrap();
        let active = download_state.active_downloads();

        let block = Block::default().borders(Borders::ALL).title(format!(
            " Active Downloads ({}/{} concurrent) ",
            active.len(),
            app.cli.concurrent
        ));
        f.render_widget(block.clone(), left_chunks[3]);

        let inner = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints((0..6).map(|_| Constraint::Length(2)).collect::<Vec<_>>())
            .split(left_chunks[3]);

        for (i, dl) in active.iter().take(6).enumerate() {
            let pct = if let Some(total) = dl.total {
                if total > 0 {
                    (dl.downloaded as f64 / total as f64 * 100.0) as u16
                } else {
                    0
                }
            } else {
                0
            };

            let speed = format_bytes_speed(dl.current_speed());
            let eta = dl
                .eta()
                .map(format_duration)
                .unwrap_or_else(|| "--:--".to_string());

            let progress_text = if let Some(total) = dl.total {
                format!("{}/{}", format_bytes(dl.downloaded), format_bytes(total))
            } else {
                format_bytes(dl.downloaded)
            };

            let name = if dl.name.len() > 30 {
                format!("{}...", &dl.name[..27])
            } else {
                dl.name.clone()
            };

            let status_icon = match dl.status {
                DownloadStatus::Downloading => "↓",
                DownloadStatus::Resolving => "◌",
                DownloadStatus::Retrying => "↻",
                _ => " ",
            };

            let retry_info = if dl.retries > 0 {
                format!(" [retry {}]", dl.retries)
            } else {
                String::new()
            };

            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(Color::Green))
                .percent(pct.min(100))
                .label(format!(
                    "{} {} | {} | {} | {} | ETA: {}{}",
                    status_icon, name, dl.host, progress_text, speed, eta, retry_info
                ));

            if i < inner.len() {
                f.render_widget(gauge, inner[i]);
            }
        }

        if active.len() > 6 && inner.len() > 5 {
            let more_text = format!("... and {} more downloading", active.len() - 6);
            let more = Paragraph::new(more_text).style(Style::default().fg(Color::DarkGray));
            f.render_widget(more, inner[5]);
        }
    } else {
        let empty = Block::default()
            .borders(Borders::ALL)
            .title(" Downloads ");
        f.render_widget(empty, left_chunks[3]);
    }

    let status = Paragraph::new(format!(" {} ", app.status))
        .style(if app.downloading {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Green)
        })
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    f.render_widget(status, left_chunks[4]);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Min(5),
        ])
        .split(main_chunks[1]);

    if let Some(ref stats) = app.stats {
        let mut stats_text = vec![
            format!("Eras: {}", stats.total_eras),
            format!("Total Tracks: {}", stats.total_tracks),
            format!("With URLs: {}", stats.tracks_with_urls),
            format!("Without URLs: {}", stats.tracks_without_urls),
            String::new(),
            "Hosts:".to_string(),
        ];
        for (host, count) in &stats.hosts {
            stats_text.push(format!("  {}: {}", host, count));
        }

        let stats_para = Paragraph::new(stats_text.join("\n"))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Tracker Stats "),
            )
            .wrap(Wrap { trim: true });
        f.render_widget(stats_para, right_chunks[0]);
    } else {
        let empty = Block::default()
            .borders(Borders::ALL)
            .title(" Tracker Stats ");
        f.render_widget(empty, right_chunks[0]);
    }

    if let Some(ref stats) = app.stats {
        let mut format_text: Vec<String> = stats
            .formats
            .iter()
            .map(|(fmt, count)| format!("{}: {}", fmt.to_uppercase(), count))
            .collect();
        if format_text.is_empty() {
            format_text.push("No formats detected".to_string());
        }

        let format_para = Paragraph::new(format_text.join("\n"))
            .block(Block::default().borders(Borders::ALL).title(" Formats "))
            .wrap(Wrap { trim: true });
        f.render_widget(format_para, right_chunks[1]);
    } else {
        let empty = Block::default().borders(Borders::ALL).title(" Formats ");
        f.render_widget(empty, right_chunks[1]);
    }

    if app.downloading || app.download_state.read().unwrap().total > 0 {
        let state = app.download_state.read().unwrap();
        let summary = state.hosts_summary();

        let mut host_text: Vec<String> = summary
            .iter()
            .map(|(host, (ok, fail, skip))| format!("{}: ✓{} ✗{} ⊘{}", host, ok, fail, skip))
            .collect();

        if host_text.is_empty() {
            host_text.push("No downloads yet".to_string());
        }

        let host_para = Paragraph::new(host_text.join("\n"))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Host Results "),
            )
            .wrap(Wrap { trim: true });
        f.render_widget(host_para, right_chunks[2]);
    } else {
        let config_text = vec![
            format!("Concurrent: {}", app.cli.concurrent),
            format!("Retries: {}", app.cli.retries),
            format!("Timeout: {}s", app.cli.timeout),
            format!("Output: {}", app.cli.output.display()),
            format!("Overwrite: {}", app.cli.overwrite),
            format!("Verify: {}", app.cli.verify),
        ];

        let config_para = Paragraph::new(config_text.join("\n"))
            .block(Block::default().borders(Borders::ALL).title(" Config "))
            .wrap(Wrap { trim: true });
        f.render_widget(config_para, right_chunks[2]);
    }

    if app.show_help {
        render_help_overlay(f);
    }
}

async fn run_tui(mut app: App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    app.load_tracker().await?;

    loop {
        terminal.draw(|f| render_ui(f, &app))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if app.show_help {
                    match key.code {
                        KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                            app.show_help = false;
                        }
                        _ => {}
                    }
                    continue;
                }

                if app.downloading {
                    if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                        app.should_quit = true;
                        break;
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') => app.should_quit = true,
                    KeyCode::Esc => app.should_quit = true,
                    KeyCode::Char('?') => app.show_help = true,
                    KeyCode::Up | KeyCode::Char('k') => {
                        let i = app.list_state.selected().unwrap_or(0);
                        if i > 0 {
                            app.list_state.select(Some(i - 1));
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let i = app.list_state.selected().unwrap_or(0);
                        if i < app.eras.len().saturating_sub(1) {
                            app.list_state.select(Some(i + 1));
                        }
                    }
                    KeyCode::PageUp => {
                        let i = app.list_state.selected().unwrap_or(0);
                        app.list_state.select(Some(i.saturating_sub(10)));
                    }
                    KeyCode::PageDown => {
                        let i = app.list_state.selected().unwrap_or(0);
                        let new_i = (i + 10).min(app.eras.len().saturating_sub(1));
                        app.list_state.select(Some(new_i));
                    }
                    KeyCode::Home => {
                        app.list_state.select(Some(0));
                    }
                    KeyCode::End => {
                        if !app.eras.is_empty() {
                            app.list_state.select(Some(app.eras.len() - 1));
                        }
                    }
                    KeyCode::Char(' ') => app.toggle_selection(),
                    KeyCode::Char('a') => app.select_all(),
                    KeyCode::Char('n') => app.deselect_all(),
                    KeyCode::Char('d') => app.show_details = !app.show_details,
                    KeyCode::Char('r') => {
                        app.status = "Reloading tracker...".to_string();
                        if let Err(e) = app.load_tracker().await {
                            app.status = format!("Reload failed: {}", e);
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Some(ref stats) = app.stats {
                            app.status = format!(
                                "Stats: {} eras, {} tracks, {} with URLs | Hosts: {}",
                                stats.total_eras,
                                stats.total_tracks,
                                stats.tracks_with_urls,
                                stats.hosts.keys().cloned().collect::<Vec<_>>().join(", ")
                            );
                        }
                    }
                    KeyCode::Enter => {
                        let download_state = app.download_state.clone();
                        let cli = app.cli.clone();
                        let tracker_data = app.tracker_data.clone();
                        let eras = app.eras.clone();
                        let client = app.client.clone();

                        app.downloading = true;

                        tokio::spawn(async move {
                            let _ = download_in_background(
                                client,
                                cli,
                                tracker_data,
                                eras,
                                download_state,
                            )
                            .await;
                        });
                    }
                    _ => {}
                }

                if app.should_quit {
                    break;
                }
            }
        }

        if app.downloading {
            let state = app.download_state.read().unwrap();
            let finished = state.completed + state.failed + state.skipped;
            if finished >= state.total && state.total > 0 {
                app.downloading = false;
                let elapsed = state.elapsed().map(format_duration).unwrap_or_default();
                let speed = format_bytes_speed(state.overall_speed());
                let total_dl = format_bytes(state.total_bytes_downloaded);
                app.status = format!(
                    "Done! ✓{} ✗{} ⊘{} | {} downloaded | {} | {}",
                    state.completed, state.failed, state.skipped, total_dl, speed, elapsed
                );
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

async fn download_in_background(
    client: Client,
    cli: Cli,
    tracker_data: Option<TrackerResponse>,
    eras: Vec<EraItem>,
    download_state: Arc<RwLock<DownloadState>>,
) -> Result<()> {
    let data = tracker_data.ok_or_else(|| anyhow!("No data"))?;

    let selected_keys: Vec<String> = eras
        .iter()
        .filter(|e| e.selected)
        .map(|e| e.key.clone())
        .collect();

    if selected_keys.is_empty() {
        return Ok(());
    }

    let mut tracks_to_download: Vec<DownloadTrack> = Vec::new();
    let mut id_counter = 0;

    for key in &selected_keys {
        if let Some(era) = data.eras.get(key) {
            if let Some(ref categories) = era.data {
                for track_list in categories.values() {
                    for track in track_list {
                        if let Some(url) = get_track_url(track) {
                            if let Some(ref format_filter) = cli.format {
                                let detected = detect_format(&url);
                                if !detected.eq_ignore_ascii_case(format_filter) {
                                    continue;
                                }
                            }

                            let host = detect_host(&url);

                            if let Some(playable) = resolve_playable_url(&client, &url).await {
                                tracks_to_download.push(DownloadTrack {
                                    id: id_counter,
                                    name: track.name.clone(),
                                    era_name: era.name.clone(),
                                    original_url: url.clone(),
                                    playable_url: playable,
                                    host,
                                });
                                id_counter += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    let total = tracks_to_download.len();
    if total == 0 {
        return Ok(());
    }

    {
        let mut state = download_state.write().unwrap();
        state.downloads = tracks_to_download
            .iter()
            .map(|t| ActiveDownload {
                id: t.id,
                name: t.name.clone(),
                era_name: t.era_name.clone(),
                host: t.host.clone(),
                url: t.playable_url.clone(),
                downloaded: 0,
                total: None,
                status: DownloadStatus::Pending,
                started_at: None,
                speed_samples: Vec::new(),
                retries: 0,
                error: None,
            })
            .collect();
        state.total = total;
        state.completed = 0;
        state.failed = 0;
        state.skipped = 0;
        state.total_bytes_downloaded = 0;
        state.started_at = Some(Instant::now());
    }

    let artist_name = cli
        .artist
        .clone()
        .or_else(|| data.name.clone())
        .unwrap_or_else(|| "Unknown Artist".to_string());

    let base_dir = cli.output.join(sanitize_filename::sanitize(&artist_name));
    std::fs::create_dir_all(&base_dir)?;

    let concurrent = cli.concurrent.clamp(1, 20);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrent));
    let mut handles = Vec::new();

    for (index, track) in tracks_to_download.into_iter().enumerate() {
        let client = client.clone();
        let base_dir = base_dir.clone();
        let download_state = download_state.clone();
        let cli = cli.clone();
        let permit = semaphore.clone().acquire_owned().await.unwrap();

        let handle = tokio::spawn(async move {
            if let Some(delay_ms) = cli.delay {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            let mut attempts = 0;
            let max_retries = cli.retries;
            let mut last_error: Option<String> = None;

            loop {
                let result = download_track_with_progress(
                    &client,
                    &track,
                    &base_dir,
                    download_state.clone(),
                    &cli,
                    index,
                )
                .await;

                match result {
                    Ok(success) => {
                        let mut state = download_state.write().unwrap();
                        if let Some(dl) = state.downloads.iter_mut().find(|d| d.id == track.id) {
                            if success {
                                dl.status = DownloadStatus::Complete;
                                state.completed += 1;
                            } else {
                                dl.status = DownloadStatus::Skipped;
                                state.skipped += 1;
                            }
                        }
                        break;
                    }
                    Err(e) => {
                        attempts += 1;
                        last_error = Some(e.to_string());

                        if attempts > max_retries {
                            let mut state = download_state.write().unwrap();
                            if let Some(dl) =
                                state.downloads.iter_mut().find(|d| d.id == track.id)
                            {
                                dl.status = DownloadStatus::Failed;
                                dl.error = last_error.clone();
                                state.failed += 1;
                            }
                            break;
                        } else {
                            {
                                let mut state = download_state.write().unwrap();
                                if let Some(dl) =
                                    state.downloads.iter_mut().find(|d| d.id == track.id)
                                {
                                    dl.status = DownloadStatus::Retrying;
                                    dl.retries = attempts;
                                }
                            }
                            tokio::time::sleep(Duration::from_secs(2_u64.pow(attempts as u32)))
                                .await;
                        }
                    }
                }
            }

            drop(permit);
        });

        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}

fn print_hosts() {
    println!("Supported file hosts:");
    println!();
    println!("  Host              Domain(s)");
    println!("  ────────────────  ─────────────────────────────────");
    println!("  Pixeldrain        pixeldrain.com");
    println!("  Pillows.su        pillows.su, pillowcase.su");
    println!("  KrakenFiles       krakenfiles.com");
    println!("  imgur.gg          imgur.gg");
    println!("  SoundCloud        soundcloud.com");
    println!("  Tidal             tidal.com (HI_RES_LOSSLESS)");
    println!("  Qobuz             qobuz.com (Max quality)");
    println!("  Froste.lol        music.froste.lol");
    println!("  JuiceWRLD API     juicewrldapi.com");
    println!();
    println!("Audio formats supported: FLAC, MP3, M4A, WAV, OGG, AAC");
}

async fn test_apis(client: &Client) {
    println!("Testing API connectivity...");
    println!();

    let tests = vec![
        ("Tracker API", format!("{}/health", API_BASE)),
        ("Tidal API 1", format!("{}/", TIDAL_APIS[0])),
        ("Tidal API 2", format!("{}/", TIDAL_APIS[1])),
        ("Tidal API 3", format!("{}/", TIDAL_APIS[2])),
        ("Qobuz API", QOBUZ_API.to_string()),
    ];

    for (name, url) in tests {
        let start = Instant::now();
        match client
            .get(&url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) => {
                let elapsed = start.elapsed();
                println!(
                    "  [OK]   {:15} {} ({:?})",
                    name,
                    resp.status(),
                    elapsed
                );
            }
            Err(e) => {
                println!("  [FAIL] {:15} {}", name, e);
            }
        }
    }
}

async fn run_batch(mut app: App) -> Result<()> {
    if !app.cli.quiet {
        println!("trackerdl v{}", VERSION);
        println!("Loading tracker {}...", app.tracker_id);
    }

    app.load_tracker().await?;

    let tracker_name = app
        .tracker_data
        .as_ref()
        .and_then(|d| d.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    if !app.cli.quiet {
        println!("Tracker: {}", tracker_name);
        println!(
            "Tab: {}",
            app.tracker_data
                .as_ref()
                .map(|d| d.current_tab.clone())
                .unwrap_or_default()
        );
        println!("Eras: {}", app.eras.len());

        if let Some(ref stats) = app.stats {
            println!(
                "Tracks: {} ({} with URLs)",
                stats.total_tracks, stats.tracks_with_urls
            );
            println!("Hosts: {:?}", stats.hosts.keys().collect::<Vec<_>>());
            println!("Formats: {:?}", stats.formats.keys().collect::<Vec<_>>());
        }
    }

    if app.cli.stats_only {
        return Ok(());
    }

    app.select_all();

    if !app.cli.quiet {
        println!(
            "\nDownloading with {} concurrent connections...",
            app.cli.concurrent
        );
    }

    app.download_selected().await?;

    let state = app.download_state.read().unwrap();
    let elapsed = state.elapsed().map(format_duration).unwrap_or_default();
    let speed = format_bytes_speed(state.overall_speed());
    let total_dl = format_bytes(state.total_bytes_downloaded);

    if !app.cli.quiet {
        println!();
        println!("Results:");
        println!("  Completed: {}", state.completed);
        println!("  Failed: {}", state.failed);
        println!("  Skipped: {}", state.skipped);
        println!("  Downloaded: {}", total_dl);
        println!("  Average Speed: {}", speed);
        println!("  Duration: {}", elapsed);

        if app.cli.verbose {
            println!();
            println!("By host:");
            for (host, (ok, fail, skip)) in state.hosts_summary() {
                println!("  {}: ✓{} ✗{} ⊘{}", host, ok, fail, skip);
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.list_hosts {
        print_hosts();
        return Ok(());
    }

    let app = App::new(cli.clone()).await?;

    if cli.test_apis {
        test_apis(&app.client).await;
        return Ok(());
    }

    if cli.no_tui {
        run_batch(app).await
    } else {
        run_tui(app).await
    }
}
