//! A running plugin: two instances on their own threads (panels answer at once even while an action runs for a
//! minute), and tickets the UI polls each frame.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use wasmtime::component::Component;

use crate::manifest::Manifest;
use crate::runtime::{Activity, Capture, Instance, State};
use crate::{Error, Result};

enum Job {
    Panel { id: String, values: String, reply: Sender<Result<String>> },
    Run { action: String, values: String, capture: Option<Capture>, reply: Sender<Result<String>> },
}

/// An answer on its way.
pub struct Ticket(Receiver<Result<String>>);

impl Ticket {
    /// The answer once it's there (`None` while waiting).
    pub fn poll(&self) -> Option<Result<String>> {
        match self.0.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(Error::Runtime("the plugin stopped".into()))),
        }
    }

    /// Waits for the answer (tests, the control channel).
    pub fn wait(self) -> Result<String> {
        self.0.recv().map_err(|_| Error::Runtime("the plugin stopped".into()))?
    }
}

pub struct Loaded {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub activity: Arc<Activity>,
    panels: Sender<Job>,
    actions: Sender<Job>,
}

fn spawn(name: String, component: Component, make: impl Fn() -> State + Send + 'static) -> Result<Sender<Job>> {
    let (tx, rx) = mpsc::channel::<Job>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            let mut inst = match Instance::new(&component, make()) {
                Ok(i) => {
                    let _ = ready_tx.send(Ok(()));
                    i
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            for job in rx {
                let (out, reply) = match job {
                    Job::Panel { id, values, reply } => (inst.panel(&id, &values), reply),
                    Job::Run { action, values, capture, reply } => (inst.run(&action, &values, capture), reply),
                };
                // After a trap (a panic, Cancel, a time limit) wasmtime won't enter that instance again: start a
                // fresh one, so the plugin keeps working. Its settings and secrets are on disk, so nothing is lost.
                if matches!(out, Err(Error::Trap(_) | Error::Cancelled))
                    || matches!(&out, Err(Error::Plugin(m)) if m.contains("took too long"))
                {
                    match Instance::new(&component, make()) {
                        Ok(i) => inst = i,
                        Err(e) => {
                            let _ = reply.send(Err(e));
                            return;
                        }
                    }
                }
                let _ = reply.send(out);
            }
        })
        .map_err(|e| Error::Runtime(e.to_string()))?;
    ready_rx.recv().map_err(|_| Error::Runtime("the plugin didn't start".into()))??;
    Ok(tx)
}

impl Loaded {
    pub fn start(manifest: Manifest, dir: PathBuf, component: Component, activity: Arc<Activity>) -> Result<Self> {
        let state = {
            let (id, perms, dir, activity) = (manifest.id.clone(), manifest.permissions.clone(), dir.clone(), activity.clone());
            move || State::new(&id, perms.clone(), dir.clone(), activity.clone())
        };
        let panels = spawn(format!("plugin-{}-panels", manifest.id), component.clone(), state.clone())?;
        let actions = spawn(format!("plugin-{}-actions", manifest.id), component, state)?;
        Ok(Self { manifest, dir, activity, panels, actions })
    }

    pub fn panel(&self, id: &str, values: &serde_json::Value) -> Ticket {
        let (reply, rx) = mpsc::channel();
        let _ = self.panels.send(Job::Panel { id: id.to_string(), values: values.to_string(), reply });
        Ticket(rx)
    }

    /// Starts an action. Only one at a time: a second while one runs is refused.
    pub fn run(&self, action: &str, values: &serde_json::Value, capture: Option<Capture>) -> Ticket {
        let (reply, rx) = mpsc::channel();
        if capture.is_some() && !self.manifest.permissions.capture {
            let _ = reply.send(Err(Error::Plugin("this plugin didn't ask to see the view".into())));
            return Ticket(rx);
        }
        if self.activity.busy.swap(true, Ordering::AcqRel) {
            let _ = reply.send(Err(Error::Plugin("it's still working on the last one".into())));
            return Ticket(rx);
        }
        if let Ok(mut p) = self.activity.progress.lock() {
            *p = (-1.0, String::new());
        }
        // A relay clears "busy" when the answer comes back, whoever polls it.
        let (inner_tx, inner_rx) = mpsc::channel::<Result<String>>();
        let activity = self.activity.clone();
        let _ = std::thread::Builder::new().name("plugin-relay".into()).spawn(move || {
            let r = inner_rx.recv().unwrap_or_else(|_| Err(Error::Runtime("the plugin stopped".into())));
            activity.busy.store(false, Ordering::Release);
            let _ = reply.send(r);
        });
        let job = Job::Run { action: action.to_string(), values: values.to_string(), capture, reply: inner_tx };
        if self.actions.send(job).is_err() {
            self.activity.busy.store(false, Ordering::Release);
        }
        Ticket(rx)
    }

    pub fn busy(&self) -> bool {
        self.activity.busy.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.activity.cancel.store(true, Ordering::Release);
    }
}
