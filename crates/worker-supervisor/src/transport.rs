use crate::{platform, Backend, Executable};
use speakmore_protocol::{read_frame_checked, write_audio, write_control, Control, Frame};
use std::{
    io,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
};

pub(crate) struct Outgoing {
    pub control: Control,
    pub audio: Option<Vec<f32>>,
}
pub(crate) enum Incoming {
    Control(Box<Control>),
    Broken,
}

pub(crate) struct Process {
    child: Option<Child>,
    _guard: Option<platform::Guard>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    sender: Option<SyncSender<Outgoing>>,
    pub receiver: Receiver<Incoming>,
    alive: Arc<AtomicBool>,
    retiring: bool,
    reaped: bool,
}
impl Process {
    pub fn spawn(executable: &Executable) -> io::Result<Self> {
        let path = &executable.path;
        if !path.is_absolute() || !std::fs::symlink_metadata(path)?.file_type().is_file() {
            return Err(io::Error::other("Invalid packaged worker"));
        }
        let mut command = Command::new(path);
        platform::configure(&mut command);
        if executable.backend == Backend::Cpu {
            command.arg("--cpu");
        }
        command
            .current_dir(
                path.parent()
                    .ok_or_else(|| io::Error::other("Invalid worker directory"))?,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let child = command.spawn()?;
        let (output, receiver) = mpsc::sync_channel(32);
        let mut process = Self {
            child: Some(child),
            _guard: None,
            writer: None,
            reader: None,
            sender: None,
            receiver,
            alive: Arc::new(AtomicBool::new(true)),
            retiring: false,
            reaped: false,
        };
        process._guard = Some(platform::Guard::attach(process.child.as_ref().unwrap())?);
        let child = process.child.as_mut().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (sender, input) = mpsc::sync_channel::<Outgoing>(2);
        process.sender = Some(sender);
        let failed = output.clone();
        let alive = process.alive.clone();
        process.writer = Some(thread::Builder::new().name("asr-pipe-write".into()).spawn(
            move || {
                while let Ok(outgoing) = input.recv() {
                    if !alive.load(Ordering::Acquire) {
                        break;
                    }
                    let result = write_control(&mut stdin, &outgoing.control).and_then(|()| {
                        if let Some(samples) = outgoing.audio {
                            let Control::Transcribe { session_id, .. } = outgoing.control else {
                                return Err(io::Error::other("Unexpected PCM"));
                            };
                            // PCM moves into this thread once; only a 16 KiB byte block
                            // is serialized at a time. Killing the child unblocks writes.
                            write_audio(&mut stdin, session_id, &samples)
                        } else {
                            Ok(())
                        }
                    });
                    if result.is_err() {
                        alive.store(false, Ordering::Release);
                        let _ = failed.try_send(Incoming::Broken);
                        break;
                    }
                }
            },
        )?);
        let alive = process.alive.clone();
        process.reader = Some(thread::Builder::new().name("asr-pipe-read".into()).spawn(
            move || {
                while alive.load(Ordering::Acquire) {
                    // A worker must never send PCM. Reject its header before allocation.
                    let frame = read_frame_checked(&mut stdout, |_, _| {
                        Err(io::Error::other("Audio in worker output"))
                    });
                    match frame {
                        Ok(Frame::Control(control)) => {
                            if output
                                .try_send(Incoming::Control(Box::new(control)))
                                .is_err()
                            {
                                break;
                            }
                        }
                        _ => {
                            let _ = output.try_send(Incoming::Broken);
                            break;
                        }
                    }
                }
                alive.store(false, Ordering::Release);
            },
        )?);
        Ok(process)
    }
    pub fn send(&self, control: Control, audio: Option<Vec<f32>>) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(Outgoing { control, audio }).is_ok())
    }
    pub fn broken(&mut self) -> bool {
        if !self.alive.load(Ordering::Acquire) {
            return true;
        }
        !matches!(self.child.as_mut().unwrap().try_wait(), Ok(None))
    }
    pub fn is_retiring(&self) -> bool {
        self.retiring
    }
    pub fn retire(&mut self) {
        if self.retiring {
            return;
        }
        self.retiring = true;
        self.alive.store(false, Ordering::Release);
        self.sender.take();
        if let Some(child) = &mut self.child {
            let _ = child.kill();
        }
    }
    // No actor waits on a pipe writer or thread join. Only join after is_finished.
    pub fn retired(&mut self) -> bool {
        if !self.retiring {
            return false;
        }
        if !self.reaped {
            self.reaped = matches!(self.child.as_mut().unwrap().try_wait(), Ok(Some(_)));
        }
        if !self.reaped
            || self.writer.as_ref().is_some_and(|t| !t.is_finished())
            || self.reader.as_ref().is_some_and(|t| !t.is_finished())
        {
            return false;
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        true
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.retire();
        if self.reaped && self.writer.is_none() && self.reader.is_none() {
            return;
        }
        let child = self.child.take();
        let writer = self.writer.take();
        let reader = self.reader.take();
        let guard = self._guard.take();
        // Destruction also works without a final tick. The UI never joins threads.
        // Normal retirement reaps before constructing another worker.
        let _ = thread::Builder::new()
            .name("asr-process-reaper".into())
            .spawn(move || {
                let _guard = guard;
                if let Some(mut child) = child {
                    let _ = child.wait();
                }
                if let Some(writer) = writer {
                    let _ = writer.join();
                }
                if let Some(reader) = reader {
                    let _ = reader.join();
                }
            });
    }
}
