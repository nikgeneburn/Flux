use crate::Application::App;
use crate::Control::Panes::LoadState;
use crate::FileSystem::Enumerate;
use crate::FileSystem::Search::{self, Query};
use crate::FileSystem::Sorting::{self, SortKey, SortSpec};
use crate::Interface::FileView::Columns::WithCommas;
use crate::Services::Workers::CancelToken;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE, WriteConsoleW};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount, GetProcessTimes};

fn Ticks(Time: FILETIME) -> u64 {
    ((Time.dwHighDateTime as u64) << 32) | Time.dwLowDateTime as u64
}

fn ProcessTimes() -> (u64, u64) {
    let (mut Creation, mut Exit, mut Kernel, mut User) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut Creation, &mut Exit, &mut Kernel, &mut User);
    }
    (Ticks(Creation), Ticks(Kernel) + Ticks(User))
}

pub fn MillisecondsSinceProcessStart() -> f64 {
    let (Creation, _) = ProcessTimes();
    (Search::NowFileTime() as u64).saturating_sub(Creation) as f64 / 10_000.0
}

pub fn MemoryCounters() -> (u64, u64) {
    let mut Counters = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut Counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS, Counters.cb);
    }
    (Counters.WorkingSetSize as u64, Counters.PrivateUsage as u64)
}

pub struct Performance {
    pub StartupMilliseconds: Option<f64>,
    pub Frames: u64,
    pub LastFrame: f64,
    pub AverageFrame: f64,
    pub MaxFrame: f64,
    pub Navigation: Option<(f64, f64, usize)>,
    pub SortTime: Option<(f64, usize)>,
    CpuSample: (Instant, u64),
    pub CpuPercent: f64,
    pub WorkingSet: u64,
    pub PrivateBytes: u64,
    pub CacheBytes: usize,
    pub Handles: u32,
    Processors: f64,
}

impl Performance {
    pub fn New() -> Performance {
        Performance {
            StartupMilliseconds: None,
            Frames: 0,
            LastFrame: 0.0,
            AverageFrame: 0.0,
            MaxFrame: 0.0,
            Navigation: None,
            SortTime: None,
            CpuSample: (Instant::now(), ProcessTimes().1),
            CpuPercent: 0.0,
            WorkingSet: 0,
            PrivateBytes: 0,
            CacheBytes: 0,
            Handles: 0,
            Processors: std::thread::available_parallelism().map(|Count| Count.get() as f64).unwrap_or(1.0),
        }
    }

    pub fn RecordFrame(&mut self, Duration: Duration) {
        let Milliseconds = Duration.as_secs_f64() * 1000.0;
        if self.StartupMilliseconds.is_none() {
            self.StartupMilliseconds = Some(MillisecondsSinceProcessStart());
        }
        self.Frames += 1;
        self.LastFrame = Milliseconds;
        self.AverageFrame = if self.Frames == 1 { Milliseconds } else { self.AverageFrame * 0.92 + Milliseconds * 0.08 };
        if self.Frames > 2 {
            self.MaxFrame = self.MaxFrame.max(Milliseconds);
        }
    }

    pub fn RecordNavigation(&mut self, Total: Duration, Enumeration: Duration, Count: usize) {
        self.Navigation = Some((Total.as_secs_f64() * 1000.0, Enumeration.as_secs_f64() * 1000.0, Count));
    }

    pub fn RecordSort(&mut self, Duration: Duration, Count: usize) {
        self.SortTime = Some((Duration.as_secs_f64() * 1000.0, Count));
    }

    pub fn Sample(&mut self, CacheBytes: usize) {
        let Now = Instant::now();
        let (_, Cpu) = ProcessTimes();
        let Wall = Now.duration_since(self.CpuSample.0).as_secs_f64();
        if Wall > 0.05 {
            self.CpuPercent = (Cpu.saturating_sub(self.CpuSample.1) as f64 / 10_000_000.0) / Wall / self.Processors * 100.0;
            self.CpuSample = (Now, Cpu);
        }
        let (WorkingSet, Private) = MemoryCounters();
        self.WorkingSet = WorkingSet;
        self.PrivateBytes = Private;
        self.CacheBytes = CacheBytes;
        let mut Handles = 0u32;
        unsafe {
            let _ = GetProcessHandleCount(GetCurrentProcess(), &mut Handles);
        }
        self.Handles = Handles;
    }

    pub fn Summary(&self) -> String {
        let mut Text = format!("Frame {:.2} ms (avg {:.2}, max {:.1})", self.LastFrame, self.AverageFrame, self.MaxFrame);
        if let Some(Startup) = self.StartupMilliseconds {
            let _ = write!(Text, " · Start {Startup:.0} ms");
        }
        if let Some((Total, Enumeration, Count)) = self.Navigation {
            let _ = write!(Text, " · Nav {Total:.1} ms ({Enumeration:.1} enum, {} items)", WithCommas(Count as u64));
        }
        if let Some((Sort, Count)) = self.SortTime {
            let _ = write!(Text, " · Sort {Sort:.1} ms/{}", WithCommas(Count as u64));
        }
        let _ = write!(
            Text,
            " · CPU {:.1}% · WS {:.1} MB · Private {:.1} MB · Thumbs {:.1} MB · {} handles",
            self.CpuPercent,
            self.WorkingSet as f64 / 1048576.0,
            self.PrivateBytes as f64 / 1048576.0,
            self.CacheBytes as f64 / 1048576.0,
            self.Handles
        );
        Text
    }
}

fn Output(Text: &str, File: Option<&PathBuf>) {
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() || GetStdHandle(STD_OUTPUT_HANDLE).is_ok() {
            if let Ok(Handle) = GetStdHandle(STD_OUTPUT_HANDLE) {
                let Wide: Vec<u16> = format!("\r\n{Text}\r\n").encode_utf16().collect();
                let _ = WriteConsoleW(Handle, &Wide, None, None);
                let _ = std::io::Write::write_all(&mut std::io::stdout(), Text.as_bytes());
            }
        }
    }
    if let Some(File) = File {
        let _ = std::fs::write(File, Text);
    }
}

fn Median(Values: &mut [f64]) -> f64 {
    Values.sort_by(|A, B| A.partial_cmp(B).unwrap_or(std::cmp::Ordering::Equal));
    Values[Values.len() / 2]
}

fn OptionValue(Arguments: &[String], Name: &str) -> Option<String> {
    Arguments.iter().position(|Item| Item.eq_ignore_ascii_case(Name)).and_then(|Index| Arguments.get(Index + 1)).cloned()
}

pub fn RunBenchmark(Arguments: &[String]) -> i32 {
    let Some(Directory) = OptionValue(Arguments, "--benchmark").map(PathBuf::from) else { return 2 };
    let Iterations: usize = OptionValue(Arguments, "--iterations").and_then(|Value| Value.parse().ok()).unwrap_or(5).max(1);
    let Query = OptionValue(Arguments, "--query").unwrap_or_else(|| "e".to_string());
    let OutputFile = OptionValue(Arguments, "--output").map(PathBuf::from);
    let mut Report = format!("Flux {} benchmark\nDirectory: {}\nIterations: {Iterations}\n\n", env!("CARGO_PKG_VERSION"), Directory.display());
    let (BaseSet, BasePrivate) = MemoryCounters();
    let Cancel = CancelToken::New();
    let mut Times = Vec::new();
    let mut Listing = None;
    for _ in 0..Iterations {
        let Started = Instant::now();
        match Enumerate::LoadDirectory(&Directory, &Cancel, &mut |_, _| {}) {
            Ok(Data) => {
                Times.push(Started.elapsed().as_secs_f64() * 1000.0);
                Listing = Some(Data);
            }
            Err(Code) => {
                Output(&format!("Enumeration failed: {}", crate::FileSystem::ErrorText(Code)), OutputFile.as_ref());
                return 1;
            }
        }
    }
    let Data = Listing.unwrap_or_default();
    let Count = Data.Entries.len();
    let First = Times[0];
    let Middle = Median(&mut Times.clone());
    let _ = writeln!(Report, "Enumerate: {} entries · first {First:.2} ms · median {Middle:.2} ms · {:.0} entries/s", WithCommas(Count as u64), Count as f64 / (Middle / 1000.0).max(1e-9));
    let (AfterSet, AfterPrivate) = MemoryCounters();
    let Footprint = Data.Text.capacity() * 2 + Data.Entries.capacity() * std::mem::size_of::<crate::FileSystem::Entry>();
    let _ = writeln!(
        Report,
        "Listing memory: {:.2} MB ({:.1} bytes/entry) · process private delta {:.2} MB · working set delta {:.2} MB",
        Footprint as f64 / 1048576.0,
        Footprint as f64 / Count.max(1) as f64,
        AfterPrivate.saturating_sub(BasePrivate) as f64 / 1048576.0,
        AfterSet.saturating_sub(BaseSet) as f64 / 1048576.0
    );
    for Key in [SortKey::Name, SortKey::Size, SortKey::Modified, SortKey::Type] {
        let mut Samples = Vec::new();
        for _ in 0..Iterations {
            let mut Order = Sorting::LiveIndices(&Data);
            let Started = Instant::now();
            Sorting::SortOrder(&Data, &mut Order, SortSpec { Key, Descending: false, FoldersFirst: true });
            Samples.push(Started.elapsed().as_secs_f64() * 1000.0);
        }
        let _ = writeln!(Report, "Sort by {:<8} median {:.2} ms", Key.Label(), Median(&mut Samples));
    }
    let Filter = Query::Parse(&Query);
    let mut Samples = Vec::new();
    let mut Matches = 0;
    for _ in 0..Iterations {
        let Started = Instant::now();
        Matches = (0..Count as u32)
            .filter(|&Index| {
                let Item = &Data.Entries[Index as usize];
                Filter.Matches(Data.Name(Index), Data.Extension(Index), Item.Attributes, Item.Size, Item.Modified)
            })
            .count();
        Samples.push(Started.elapsed().as_secs_f64() * 1000.0);
    }
    let _ = writeln!(Report, "Filter \"{Query}\": {} matches · median {:.2} ms", WithCommas(Matches as u64), Median(&mut Samples));
    let Started = Instant::now();
    let mut Found = 0usize;
    let Statistics = Search::Run(&Directory, &Filter, true, &Cancel, &mut |Chunk| Found += Chunk.Entries.len());
    let _ = writeln!(
        Report,
        "Recursive search \"{Query}\": {} matches in {} folders / {} entries · {:.1} ms ({:.0} entries/s)",
        WithCommas(Found as u64),
        WithCommas(Statistics.Directories),
        WithCommas(Statistics.Scanned),
        Started.elapsed().as_secs_f64() * 1000.0,
        Statistics.Scanned as f64 / Started.elapsed().as_secs_f64().max(1e-9)
    );
    Output(&Report, OutputFile.as_ref());
    0
}

pub fn GenerateFiles(Arguments: &[String]) -> i32 {
    let Some(Directory) = OptionValue(Arguments, "--generate").map(PathBuf::from) else { return 2 };
    let Count: usize = OptionValue(Arguments, "--count").and_then(|Value| Value.parse().ok()).unwrap_or(100_000);
    if std::fs::create_dir_all(&Directory).is_err() {
        return 1;
    }
    let Started = Instant::now();
    let Extensions = ["txt", "rs", "png", "jpg", "pdf", "docx", "json", "log", "dll", "exe"];
    let mut Created = 0usize;
    for Index in 0..Count {
        let Name = if Index % 50 == 0 { format!("Folder {Index:07}") } else { format!("file-{:07}-{}.{}", Index, ["alpha", "beta", "gamma", "delta", "ünïcødé", "日本"][Index % 6], Extensions[Index % Extensions.len()]) };
        let Target = Directory.join(Name);
        let Result = if Index % 50 == 0 { std::fs::create_dir(&Target) } else { std::fs::write(&Target, Index.to_le_bytes()) };
        if Result.is_ok() {
            Created += 1;
        }
    }
    Output(&format!("Created {} entries in {} ({:.1} s)", WithCommas(Created as u64), Directory.display(), Started.elapsed().as_secs_f64()), None);
    0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BenchPhase {
    Waiting,
    Scrolling,
    Idle,
    Done,
}

pub struct UiBenchmark {
    pub Directory: PathBuf,
    pub ExitCode: i32,
    Output: Option<PathBuf>,
    Phase: BenchPhase,
    Started: Instant,
    ListingMilliseconds: f64,
    Frames: Vec<f64>,
    Steps: usize,
    IdleStarted: Option<(Instant, u64, u64)>,
    LastFrame: u64,
}

impl UiBenchmark {
    pub fn FromArguments(Arguments: &[String]) -> Option<UiBenchmark> {
        let Directory = OptionValue(Arguments, "--bench-ui")?;
        Some(UiBenchmark {
            Directory: PathBuf::from(Directory),
            ExitCode: 1,
            Output: OptionValue(Arguments, "--output").map(PathBuf::from),
            Phase: BenchPhase::Waiting,
            Started: Instant::now(),
            ListingMilliseconds: 0.0,
            Frames: Vec::new(),
            Steps: 0,
            IdleStarted: None,
            LastFrame: 0,
        })
    }

    pub fn Begin(&mut self) {
        self.Started = Instant::now();
    }
}

impl App {
    pub fn BenchStep(&mut self) {
        let Some(mut Bench) = self.Bench.take() else { return };
        match Bench.Phase {
            BenchPhase::Waiting => {
                if self.Tab(0).State == LoadState::Ready && !self.Tab(0).Order.is_empty() {
                    Bench.ListingMilliseconds = MillisecondsSinceProcessStart();
                    Bench.Phase = BenchPhase::Scrolling;
                    self.Invalidate();
                } else if let LoadState::Failed(Message) = &self.Tab(0).State {
                    Output(&format!("UI benchmark failed: {Message}"), Bench.Output.as_ref());
                    Bench.Phase = BenchPhase::Done;
                    crate::Platform::Close(self.Window);
                }
            }
            BenchPhase::Scrolling => {
                if self.Performance.Frames != Bench.LastFrame {
                    Bench.LastFrame = self.Performance.Frames;
                    if Bench.Steps > 0 {
                        Bench.Frames.push(self.Performance.LastFrame);
                    }
                    Bench.Steps += 1;
                    let Page = crate::Interface::FileView::PageExtent(self, 0);
                    let (Content, Viewport) = crate::Interface::FileView::Extents(self, 0);
                    let Tab = self.TabMut(0);
                    let Next = if Tab.Scroll.Offset + Viewport >= Content - 1.0 { 0.0 } else { Tab.Scroll.Offset + Page };
                    Tab.Scroll.Jump(Next);
                    if Bench.Steps > 240 {
                        Bench.Phase = BenchPhase::Idle;
                        self.Performance.Sample(self.Media.CacheBytes());
                        Bench.IdleStarted = Some((Instant::now(), ProcessTimes().1, self.Performance.Frames));
                        self.SetPerformanceTimer(true);
                    }
                    self.Invalidate();
                }
            }
            BenchPhase::Idle => {}
            BenchPhase::Done => {}
        }
        self.Bench = Some(Bench);
    }

    pub fn BenchTick(&mut self) -> bool {
        let Count = self.Tab(0).Listing.Len();
        let Some(Bench) = &mut self.Bench else { return false };
        if Bench.Phase != BenchPhase::Idle {
            return false;
        }
        let Some((IdleStart, IdleCpu, IdleFrames)) = Bench.IdleStarted else { return false };
        if IdleStart.elapsed() < Duration::from_secs(4) {
            return false;
        }
        let IdleCpuPercent = (ProcessTimes().1.saturating_sub(IdleCpu) as f64 / 10_000_000.0) / IdleStart.elapsed().as_secs_f64() * 100.0;
        let mut Frames = Bench.Frames.clone();
        Frames.sort_by(|A, B| A.partial_cmp(B).unwrap_or(std::cmp::Ordering::Equal));
        let Average = Frames.iter().sum::<f64>() / Frames.len().max(1) as f64;
        let Percentile = |Fraction: f64| Frames.get(((Frames.len() as f64 * Fraction) as usize).min(Frames.len().saturating_sub(1))).copied().unwrap_or(0.0);
        let (WorkingSet, Private) = MemoryCounters();
        let Report = format!(
            "Flux {} UI benchmark\nDirectory: {} ({} entries)\nFirst frame after process start: {:.1} ms\nListing complete and painted: {:.1} ms after process start\nNavigation: {}\nScroll frames: {} · average {:.3} ms · p50 {:.3} ms · p95 {:.3} ms · max {:.3} ms\nIdle CPU over {:.1} s: {:.3}% of one core · {} frames painted while idle\nWorking set: {:.1} MB · Private bytes: {:.1} MB · Thumbnail cache {:.1} MB\n",
            env!("CARGO_PKG_VERSION"),
            Bench.Directory.display(),
            WithCommas(Count as u64),
            self.Performance.StartupMilliseconds.unwrap_or(0.0),
            Bench.ListingMilliseconds,
            self.Performance.Navigation.map(|(Total, Enumeration, Items)| format!("{Total:.1} ms total, {Enumeration:.1} ms enumeration, {} items", WithCommas(Items as u64))).unwrap_or_default(),
            Frames.len(),
            Average,
            Percentile(0.5),
            Percentile(0.95),
            Frames.last().copied().unwrap_or(0.0),
            IdleStart.elapsed().as_secs_f64(),
            IdleCpuPercent,
            self.Performance.Frames - IdleFrames,
            WorkingSet as f64 / 1048576.0,
            Private as f64 / 1048576.0,
            self.Media.CacheBytes() as f64 / 1048576.0
        );
        Output(&Report, Bench.Output.as_ref());
        Bench.Phase = BenchPhase::Done;
        Bench.ExitCode = 0;
        true
    }
}
