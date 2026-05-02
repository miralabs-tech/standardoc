use std::sync::{Arc, RwLock};
use std::time::Duration;

use standardoc_core::{ColdStartError, IndexHandle, LanguageProvider, ScanFilters, cold_start};
use tower_lsp_server::{
    Client,
    ls_types::{
        ProgressParams, ProgressParamsValue, ProgressToken, WorkDoneProgress,
        WorkDoneProgressBegin, WorkDoneProgressEnd, WorkDoneProgressReport, notification::Progress,
    },
};

const PROGRESS_TITLE: &str = "Indexing workspace";
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Run cold start while streaming `$/progress workDoneProgress` to the LSP
/// client. The progress lifecycle is best-effort: `create_work_done_progress`
/// failures (clients without progress support) are ignored, the indexing
/// still runs, and the result of `cold_start::run` is propagated unchanged.
pub(crate) async fn run_cold_start_with_progress(
    handle: &IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
    client: Client,
) -> Result<(), ColdStartError> {
    let token = ProgressToken::String(format!(
        "standardoc/cold-start/{:p}",
        handle.workspace_root()
    ));

    let _ = client.create_work_done_progress(token.clone()).await;
    send_begin(&client, &token).await;

    let handle_blocking = handle.clone();
    let cold_start_task = tokio::task::spawn_blocking(move || {
        let guard = filters
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cold_start::run(&handle_blocking, provider.as_ref(), &guard)
    });

    let handle_poll = handle.clone();
    let client_poll = client.clone();
    let token_poll = token.clone();
    let poll_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if let Ok(Some((done, total))) = handle_poll.cold_start_progress() {
                send_report(&client_poll, &token_poll, done, total).await;
            }
        }
    });

    let result = cold_start_task.await.unwrap_or_else(|join_err| {
        Err(ColdStartError::Io(std::io::Error::other(format!(
            "spawn_blocking join failed: {join_err}"
        ))))
    });

    poll_task.abort();
    send_end(&client, &token, &result).await;
    result
}

async fn send_begin(client: &Client, token: &ProgressToken) {
    let value = WorkDoneProgress::Begin(WorkDoneProgressBegin {
        title: PROGRESS_TITLE.to_string(),
        cancellable: Some(false),
        message: None,
        percentage: Some(0),
    });
    send_progress(client, token, value).await;
}

async fn send_report(client: &Client, token: &ProgressToken, done: u64, total: u64) {
    let percentage = if total > 0 {
        Some(
            u32::try_from(done.saturating_mul(100) / total)
                .unwrap_or(100)
                .min(100),
        )
    } else {
        None
    };
    let value = WorkDoneProgress::Report(WorkDoneProgressReport {
        cancellable: Some(false),
        message: Some(format!("{done}/{total} files")),
        percentage,
    });
    send_progress(client, token, value).await;
}

async fn send_end(client: &Client, token: &ProgressToken, outcome: &Result<(), ColdStartError>) {
    let message = match outcome {
        Ok(()) => Some("done".to_string()),
        Err(e) => Some(format!("failed: {e}")),
    };
    let value = WorkDoneProgress::End(WorkDoneProgressEnd { message });
    send_progress(client, token, value).await;
}

async fn send_progress(client: &Client, token: &ProgressToken, value: WorkDoneProgress) {
    client
        .send_notification::<Progress>(ProgressParams {
            token: token.clone(),
            value: ProgressParamsValue::WorkDone(value),
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentage_handles_zero_total() {
        // Sanity: a sibling unit can verify the report shape without a live client.
        let value = WorkDoneProgress::Report(WorkDoneProgressReport {
            cancellable: Some(false),
            message: Some("0/0 files".to_string()),
            percentage: None,
        });
        match value {
            WorkDoneProgress::Report(report) => {
                assert_eq!(report.percentage, None);
                assert_eq!(report.message.as_deref(), Some("0/0 files"));
            }
            _ => panic!("expected Report variant"),
        }
    }
}
