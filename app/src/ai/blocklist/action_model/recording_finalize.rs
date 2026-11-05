use std::path::{Path, PathBuf};
use std::time::Duration;

use ai::agent::action_result::{RecordingStopped, StopRecordingResult};
use futures::channel::oneshot;
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity};

pub(crate) use super::recording_controller::FinalizeReason;
use super::recording_controller::{
    ActiveRecording, FinalizationClaim, FinalizedRecording, RecordingController,
    StopRecordingControllerError,
};
use crate::ai::agent::conversation::AIConversationId;

const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// A handle to the canonical result owned by `RecordingController`.
///
/// `Pending` subscribes to work already owned by the controller; dropping the
/// receiver does not cancel stop. `Ready` exposes the retained result
/// after that work has completed. Both variants carry the *actual*
/// [`FinalizeReason`] that drove the work — callers that only joined an
/// in-progress finalization still learn why it ran, rather than the reason
/// they claimed when joining.
pub(crate) enum RecordingFinalization {
    Pending(oneshot::Receiver<FinalizedRecording>),
    Ready(FinalizedRecording),
}

impl RecordingFinalization {
    pub(crate) async fn resolve(self) -> FinalizedRecording {
        match self {
            RecordingFinalization::Pending(receiver) => receiver.await.unwrap_or_else(|_| {
                (
                    StopRecordingResult::Error(
                        "Recording finalization ended without producing a result.".to_string(),
                    ),
                    // The result channel closed before delivering a result, so the
                    // real trigger is unknown and distinct from an ffmpeg crash.
                    FinalizeReason::FinalizationDropped,
                )
            }),
            RecordingFinalization::Ready(ready) => ready,
        }
    }
}

async fn validated_recording(
    raw_path: &Path,
    processed_path: Option<PathBuf>,
) -> Result<(PathBuf, Duration, Option<PathBuf>), computer_use::RecordingError> {
    if let Some(processed_path) = processed_path {
        match computer_use::finalized_video_duration(&processed_path).await {
            Ok(duration) => {
                return Ok((processed_path.clone(), duration, Some(processed_path)));
            }
            Err(error) => {
                log::warn!(
                    "Recording cut/overlay output validation failed; keeping original: {error}"
                );
                let _ = std::fs::remove_file(processed_path);
            }
        }
    }

    let duration = computer_use::finalized_video_duration(raw_path).await?;
    Ok((raw_path.to_path_buf(), duration, None))
}

/// Stops capture, keeps the finalized file on disk, and produces the result retained by the
/// controller for all current and future callers.
async fn finalize_recording(
    recording: ActiveRecording,
    reason: FinalizeReason,
    should_persist: bool,
) -> StopRecordingResult {
    // A no-persist finalization discards the recording: it drops the whole `ActiveRecording`
    // (kill-on-drops ffmpeg, removes the partial capture). The reason distinguishes an
    // agent-requested discard (`Discarded`) from a conversation cancellation (`Cancelled`),
    // which the agent turn treats differently. This check comes first so it holds even when no
    // action group was committed.
    if !should_persist {
        drop(recording);
        return match reason {
            FinalizeReason::StoppedByAgent => StopRecordingResult::Discarded,
            _ => StopRecordingResult::Cancelled,
        };
    }
    if recording.actions.is_empty() {
        drop(recording);
        return StopRecordingResult::Error(
            "Recording contained no committed actions; no video was saved.".to_string(),
        );
    }
    let ActiveRecording {
        handle,
        actions,
        frame_rate,
        ..
    } = recording;
    let recorder = computer_use::create_recorder();
    let output = match recorder.stop(handle).await {
        Ok(output) => output,
        Err(error) => return StopRecordingResult::Error(error.to_string()),
    };

    let local_path = output.path.clone();

    // Apply the post-stop smart cut (keep real action windows at 1x, drop blocked/thinking gaps)
    // and burn the remapped overlay pills into the video. Best-effort: on any failure the
    // original 1x capture is kept unannotated (a no-cut video beats no video). The cut/overlay
    // file, when produced, is a sibling of the mp4.
    let processed_path = match computer_use::post_process_recording(
        &local_path,
        &actions,
        (output.width, output.height),
        output.duration,
        frame_rate,
    )
    .await
    {
        Ok(path) if path != local_path => Some(path),
        Ok(_) => None,
        Err(error) => {
            log::warn!("Recording cut/overlay burn-in failed; keeping original: {error}");
            None
        }
    };
    let (video_path, duration, overlay_path) =
        match validated_recording(&local_path, processed_path).await {
            Ok(validated) => validated,
            Err(error) => {
                let _ = std::fs::remove_file(&local_path);
                let _ = std::fs::remove_file(local_path.with_extension("log"));
                return StopRecordingResult::Error(error.to_string());
            }
        };

    let _ = std::fs::remove_file(local_path.with_extension("log"));
    if overlay_path.is_some() {
        let _ = std::fs::remove_file(&local_path);
    }

    let size_bytes = match std::fs::metadata(&video_path) {
        Ok(metadata) => metadata.len() as i64,
        Err(error) => return StopRecordingResult::Error(error.to_string()),
    };
    StopRecordingResult::Success(RecordingStopped {
        artifact_uid: video_path.display().to_string(),
        duration,
        width_px: output.width as i32,
        height_px: output.height as i32,
        size_bytes,
        completion_status: output.completion_status,
        termination_reason: reason.termination_reason(output.completion_status),
    })
}

/// Runs finalization independently of any action future and stores its result
/// on the controller before waking subscribers. The `reason` is forwarded to
/// [`RecordingController::complete_finalization`] so waiters that only joined
/// this work receive the actual reason it ran, not the reason they claimed.
fn spawn_finalize(
    recording: ActiveRecording,
    reason: FinalizeReason,
    should_persist: bool,
    ctx: &mut ModelContext<RecordingController>,
) {
    let recording_id = recording.id.clone();
    ctx.spawn(
        finalize_recording(recording, reason, should_persist),
        move |controller, result, _ctx| {
            controller.complete_finalization(&recording_id, result, reason);
        },
    );
}

/// Converts an atomic controller claim into a result handle. Only the caller
/// that receives `Claimed` starts work; concurrent and later callers subscribe
/// to the in-flight operation or receive its retained result.
fn start_or_join_finalization<T: Entity>(
    claim: FinalizationClaim,
    reason: FinalizeReason,
    should_persist: bool,
    ctx: &mut ModelContext<T>,
) -> Option<RecordingFinalization> {
    match claim {
        FinalizationClaim::Claimed {
            recording,
            result_receiver,
        } => {
            RecordingController::handle(ctx).update(ctx, |_controller, ctx| {
                spawn_finalize(*recording, reason, should_persist, ctx);
            });
            Some(RecordingFinalization::Pending(result_receiver))
        }
        FinalizationClaim::InProgress(receiver) => Some(RecordingFinalization::Pending(receiver)),
        FinalizationClaim::Finished(result) => Some(RecordingFinalization::Ready(result)),
        FinalizationClaim::NotFound => None,
    }
}

/// Starts or joins finalization for an explicit `StopRecording` request.
///
/// The returned handle only observes controller-owned work. The stop executor
/// decides when a retained result has been delivered and can be consumed.
pub(crate) fn finalize_recording_by_id<T: Entity>(
    recording_id: &str,
    reason: FinalizeReason,
    should_persist: bool,
    ctx: &mut ModelContext<T>,
) -> Result<RecordingFinalization, StopRecordingControllerError> {
    let claim = RecordingController::handle(ctx).update(ctx, |controller, _| {
        controller.claim_finalization_by_id(recording_id)
    });
    start_or_join_finalization(claim, reason, should_persist, ctx).ok_or_else(|| {
        StopRecordingControllerError::RecordingNotFound {
            recording_id: recording_id.to_string(),
        }
    })
}
/// Starts or joins finalization for this conversation.
///
/// Finalization itself is spawned on the recording controller, so dropping the
/// returned handle does not cancel stop work. The driver awaits the
/// handle before teardown; conversation cancellation only observes it for
/// logging because cancellation must remain synchronous.
pub(crate) fn finalize_recording_for_conversation<T: Entity>(
    conversation_id: AIConversationId,
    reason: FinalizeReason,
    should_persist: bool,
    ctx: &mut ModelContext<T>,
) -> Option<RecordingFinalization> {
    // The recording controller is always registered in production
    // (`app/src/lib.rs`). Guard here so the conversation-cancellation and
    // driver-teardown paths never panic in test harnesses that don't register
    // the singleton — there is simply nothing to finalize in that case.
    if !ctx.has_singleton_model::<RecordingController>() {
        return None;
    }
    let claim = RecordingController::handle(ctx).update(ctx, |controller, _| {
        controller.claim_finalization_for_conversation(conversation_id)
    })?;
    start_or_join_finalization(claim, reason, should_persist, ctx)
}

/// Polls the active ffmpeg process until it exits or another path claims it.
///
/// Each timer schedules the next one only while this recording remains active.
/// Stop, cancellation, and driver teardown move it to `Finalizing`, at which
/// point this watcher observes that it is no longer active and ends.
pub(crate) fn spawn_recording_exit_watcher(
    recording_id: String,
    ctx: &mut ModelContext<RecordingController>,
) {
    ctx.spawn(
        async move {
            Timer::after(EXIT_POLL_INTERVAL).await;
        },
        move |controller, (), ctx| match controller.poll_active_exit(&recording_id) {
            Some(exit_kind) => {
                if let FinalizationClaim::Claimed { recording, .. } =
                    controller.claim_finalization_by_id(&recording_id)
                {
                    let reason = match exit_kind {
                        computer_use::RecordingExitKind::LimitReached => {
                            FinalizeReason::LimitReached
                        }
                        computer_use::RecordingExitKind::Crashed => FinalizeReason::FfmpegExited,
                    };
                    spawn_finalize(*recording, reason, true, ctx);
                }
            }
            None if controller.active_recording_id() == Some(recording_id.as_str()) => {
                spawn_recording_exit_watcher(recording_id, ctx);
            }
            None => {}
        },
    );
}

#[cfg(test)]
#[path = "recording_finalize_tests.rs"]
mod tests;
