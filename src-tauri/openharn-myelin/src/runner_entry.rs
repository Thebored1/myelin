//! Public runner entrypoint kept separate from the state-machine implementation.

use crate::protocol::{ChatRequest, Out};
use crate::runner::run_loop;
use crate::server::Pending;
use tokio::sync::{mpsc, watch};

pub(crate) async fn run(
    request: ChatRequest,
    events: mpsc::Sender<Out>,
    pending: Pending,
    cancel: watch::Receiver<bool>,
) {
    run_loop(request, events, pending, cancel).await;
}
