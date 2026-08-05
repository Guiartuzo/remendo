//! Gerrit access on a background thread, delivered to the UI over a channel.
//!
//! The render loop is synchronous and must never block on the network, but
//! Remendo introduces **no async runtime** (specs/gerrit-client). A worker
//! thread owning a blocking client gives the same non-blocking UI for a
//! fraction of the machinery — the pattern is lifted from vybim's
//! `lsp/transport.rs`.
//!
//! ```text
//!    UI thread                         worker thread
//!    ─────────                         ─────────────
//!    request(LoadChange) ──┐
//!    render …              ├── mpsc ──▶ blocking GerritApi call
//!    render …              │                  │
//!    poll() -> None        │                  │
//!    render …              └◀── mpsc ─────────┘
//!    poll() -> Some(ChangeLoaded)
//! ```

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use super::GerritError;
use super::api::{GerritApi, ReviewInput};
use super::load::{LoadedChange, load_change};

/// Work the UI asks the Gerrit thread to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GerritRequest {
    /// Fetch a change and assemble its triagable threads.
    LoadChange { change_id: String },
    /// Issue the single batched review post. Finalize only.
    PostReview {
        change_id: String,
        review: Box<ReviewInput>,
    },
}

/// What the Gerrit thread sends back.
#[derive(Debug)]
pub enum GerritEvent {
    ChangeLoaded(Box<LoadedChange>),
    ReviewPosted,
    Failed(GerritError),
}

/// A handle to the Gerrit worker thread.
///
/// Dropping it closes the request channel, which ends the worker loop; [`join`]
/// waits for it to finish.
///
/// [`join`]: GerritWorker::join
#[derive(Debug)]
pub struct GerritWorker {
    requests: Option<Sender<GerritRequest>>,
    events: Receiver<GerritEvent>,
    handle: Option<JoinHandle<()>>,
}

impl GerritWorker {
    /// Start a worker owning `api`.
    ///
    /// `api` moves onto the worker thread, so the blocking client is never
    /// touched from the UI thread — the type system enforces what the spec asks
    /// for rather than a convention doing it.
    pub fn spawn<A: GerritApi + Send + 'static>(api: A) -> Self {
        let (request_tx, request_rx) = channel::<GerritRequest>();
        let (event_tx, event_rx) = channel::<GerritEvent>();

        let handle = std::thread::Builder::new()
            .name("remendo-gerrit".into())
            .spawn(move || serve(api, &request_rx, &event_tx))
            .expect("spawning the Gerrit worker thread");

        Self {
            requests: Some(request_tx),
            events: event_rx,
            handle: Some(handle),
        }
    }

    /// Queue work. Returns `false` if the worker has stopped.
    pub fn request(&self, request: GerritRequest) -> bool {
        self.requests
            .as_ref()
            .is_some_and(|tx| tx.send(request).is_ok())
    }

    /// Take one completed event, or `None` if none is ready.
    ///
    /// Never blocks — this is what the render loop calls each frame.
    /// Both `Empty` and `Disconnected` are `None`: a frame with nothing ready
    /// and a frame after the worker stopped look the same to the render loop,
    /// which has nothing different to do about either.
    pub fn poll(&self) -> Option<GerritEvent> {
        self.events.try_recv().ok()
    }

    /// Block until the next event arrives. For tests and for the startup load,
    /// where there is nothing to render yet.
    pub fn wait(&self) -> Option<GerritEvent> {
        self.events.recv().ok()
    }

    /// Stop the worker and wait for it to finish.
    pub fn join(mut self) {
        self.shutdown();
    }

    /// Drop the request sender so the worker's loop ends, then join it.
    fn shutdown(&mut self) {
        self.requests.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for GerritWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The worker loop: serve requests until the UI drops its sender.
fn serve<A: GerritApi>(api: A, requests: &Receiver<GerritRequest>, events: &Sender<GerritEvent>) {
    while let Ok(request) = requests.recv() {
        let event = match request {
            GerritRequest::LoadChange { change_id } => match load_change(&api, &change_id) {
                Ok(loaded) => GerritEvent::ChangeLoaded(Box::new(loaded)),
                Err(err) => GerritEvent::Failed(err),
            },
            GerritRequest::PostReview { change_id, review } => {
                match api.post_review(&change_id, &review) {
                    Ok(()) => GerritEvent::ReviewPosted,
                    Err(err) => GerritEvent::Failed(err),
                }
            }
        };
        // A closed event channel means the UI is gone; stop rather than spin.
        if events.send(event).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerrit::{Comment, FakeGerrit};

    const CHANGE_BODY: &str = r#")]}'
{"project":"proj","branch":"main","current_revision":"sha3",
 "revisions":{"sha3":{"_number":3,"ref":"refs/changes/45/12345/3"}}}"#;

    fn comment(id: &str) -> Comment {
        serde_json::from_value(serde_json::json!({
            "id": id, "patch_set": 3, "unresolved": true, "line": 1,
        }))
        .unwrap()
    }

    fn worker() -> GerritWorker {
        let api = FakeGerrit::from_change_json(CHANGE_BODY)
            .unwrap()
            .with_comments("src/a.rs", vec![comment("c1")]);
        GerritWorker::spawn(api)
    }

    #[test]
    fn a_load_request_comes_back_as_an_event() {
        let worker = worker();
        assert!(worker.request(GerritRequest::LoadChange {
            change_id: "12345".into()
        }));
        match worker.wait().expect("an event arrives") {
            GerritEvent::ChangeLoaded(loaded) => {
                assert_eq!(loaded.change.branch, "main");
                assert_eq!(loaded.threads.threads.len(), 1);
            }
            other => panic!("expected ChangeLoaded, got {other:?}"),
        }
    }

    /// The requirement this whole module exists for: the caller must be able to
    /// ask, keep rendering, and check back — never blocking on the network.
    #[test]
    fn polling_before_a_result_is_ready_does_not_block() {
        let worker = worker();
        // Nothing requested yet, so there is certainly nothing to take.
        assert!(worker.poll().is_none());

        worker.request(GerritRequest::LoadChange {
            change_id: "12345".into(),
        });
        // Simulate frames: poll returns immediately whether or not work is done.
        let mut frames = 0;
        let loaded = loop {
            if let Some(event) = worker.poll() {
                break event;
            }
            frames += 1;
            assert!(frames < 100_000, "poll never completed");
        };
        assert!(matches!(loaded, GerritEvent::ChangeLoaded(_)));
    }

    #[test]
    fn a_failure_arrives_as_an_event_rather_than_a_panic() {
        let worker = GerritWorker::spawn(FakeGerrit::default());
        worker.request(GerritRequest::LoadChange {
            change_id: "98765".into(),
        });
        match worker.wait().expect("an event arrives") {
            GerritEvent::Failed(err) => assert!(err.to_string().contains("98765")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn requests_are_served_in_order() {
        let worker = worker();
        for _ in 0..3 {
            worker.request(GerritRequest::LoadChange {
                change_id: "12345".into(),
            });
        }
        for _ in 0..3 {
            assert!(matches!(
                worker.wait().expect("an event"),
                GerritEvent::ChangeLoaded(_)
            ));
        }
    }

    #[test]
    fn a_review_post_reports_completion() {
        let worker = worker();
        worker.request(GerritRequest::PostReview {
            change_id: "12345".into(),
            review: Box::new(ReviewInput::default()),
        });
        assert!(matches!(
            worker.wait().expect("an event"),
            GerritEvent::ReviewPosted
        ));
    }

    #[test]
    fn dropping_the_worker_stops_the_thread() {
        let worker = worker();
        worker.join(); // returns only once the thread has ended
    }

    #[test]
    fn requesting_after_shutdown_is_reported_rather_than_panicking() {
        let mut worker = worker();
        worker.shutdown();
        assert!(!worker.request(GerritRequest::LoadChange {
            change_id: "12345".into()
        }));
    }
}
