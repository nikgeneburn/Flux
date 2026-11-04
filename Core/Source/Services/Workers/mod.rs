use crate::Application::Event;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryIter, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadDescription, SetThreadStackGuarantee};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::PCWSTR;

pub const WakeMessage: u32 = WM_APP + 1;

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn New() -> Self {
        Self::default()
    }

    pub fn Cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn IsCancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone)]
pub struct EventSink {
    Sender: Sender<Event>,
    Window: isize,
    Pending: Arc<AtomicBool>,
}

pub struct EventQueue {
    Receiver: Receiver<Event>,
    Pending: Arc<AtomicBool>,
}

pub fn CreateEventChannel(Window: HWND) -> (EventSink, EventQueue) {
    let (Sender, Receiver) = channel();
    let Pending = Arc::new(AtomicBool::new(false));
    (EventSink { Sender, Window: Window.0 as isize, Pending: Pending.clone() }, EventQueue { Receiver, Pending })
}

impl EventSink {
    pub fn Send(&self, Item: Event) {
        if self.Sender.send(Item).is_ok() && !self.Pending.swap(true, Ordering::AcqRel) {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.Window as *mut _)), WakeMessage, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl EventQueue {
    pub fn Drain(&self) -> TryIter<'_, Event> {
        self.Pending.store(false, Ordering::Release);
        self.Receiver.try_iter()
    }
}

pub fn Spawn<F: FnOnce() + Send + 'static>(Name: &str, Work: F) -> std::io::Result<JoinHandle<()>> {
    let Description: Vec<u16> = format!("Flux {Name}").encode_utf16().chain(std::iter::once(0)).collect();
    std::thread::Builder::new().name(Name.to_string()).stack_size(1 << 20).spawn(move || {
        unsafe {
            let mut Guarantee = 64 * 1024u32;
            let _ = SetThreadStackGuarantee(&mut Guarantee);
            let _ = SetThreadDescription(GetCurrentThread(), PCWSTR(Description.as_ptr()));
        }
        Work();
    })
}

type Job = Box<dyn FnOnce() + Send + 'static>;

struct PoolState {
    Queue: Mutex<PoolQueue>,
    Signal: Condvar,
}

struct PoolQueue {
    Jobs: VecDeque<Job>,
    Idle: usize,
    Threads: usize,
    Shutdown: bool,
}

pub struct Pool {
    Name: &'static str,
    MaxThreads: usize,
    State: Arc<PoolState>,
}

impl Pool {
    pub fn New(Name: &'static str, MaxThreads: usize) -> Self {
        Self {
            Name,
            MaxThreads: MaxThreads.max(1),
            State: Arc::new(PoolState {
                Queue: Mutex::new(PoolQueue { Jobs: VecDeque::new(), Idle: 0, Threads: 0, Shutdown: false }),
                Signal: Condvar::new(),
            }),
        }
    }

    pub fn Submit<F: FnOnce() + Send + 'static>(&self, Work: F) {
        let mut Queue = self.State.Queue.lock().unwrap_or_else(|Poisoned| Poisoned.into_inner());
        Queue.Jobs.push_back(Box::new(Work));
        if Queue.Idle == 0 && Queue.Threads < self.MaxThreads {
            Queue.Threads += 1;
            let State = self.State.clone();
            if Spawn(self.Name, move || PoolLoop(State)).is_err() {
                Queue.Threads -= 1;
            }
        }
        drop(Queue);
        self.State.Signal.notify_one();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        if let Ok(mut Queue) = self.State.Queue.lock() {
            Queue.Shutdown = true;
            Queue.Jobs.clear();
        }
        self.State.Signal.notify_all();
    }
}

fn PoolLoop(State: Arc<PoolState>) {
    loop {
        let Work = {
            let mut Queue = State.Queue.lock().unwrap_or_else(|Poisoned| Poisoned.into_inner());
            loop {
                if Queue.Shutdown {
                    return;
                }
                if let Some(Work) = Queue.Jobs.pop_front() {
                    break Work;
                }
                Queue.Idle += 1;
                Queue = State.Signal.wait(Queue).unwrap_or_else(|Poisoned| Poisoned.into_inner());
                Queue.Idle -= 1;
            }
        };
        Work();
    }
}

static NextIdentifier: AtomicU64 = AtomicU64::new(1);

pub fn NewIdentifier() -> u64 {
    NextIdentifier.fetch_add(1, Ordering::Relaxed)
}
