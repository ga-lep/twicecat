//! Mapping of the SDK's sync service state to the public [`SyncState`].

use matrix_sdk_ui::sync_service::State;

use crate::api::SyncState;

/// Maps an SDK state to the API state.
///
/// | SDK `State`  | API `SyncState`                     |
/// |--------------|-------------------------------------|
/// | `Idle`       | `Idle`                              |
/// | `Running`    | `Running`                           |
/// | `Offline`    | `Offline`                           |
/// | `Terminated` | `Terminated`                        |
/// | `Error(_)`   | `Error("sync failed")` (fixed text) |
///
/// The SDK error text is not forwarded: it can carry server-controlled data.
pub(crate) fn map_state(state: &State) -> SyncState {
    match state {
        State::Idle => SyncState::Idle,
        State::Running => SyncState::Running,
        State::Offline => SyncState::Offline,
        State::Terminated => SyncState::Terminated,
        State::Error(_) => SyncState::Error("sync failed".into()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use matrix_sdk_ui::sync_service::Error;

    use super::*;

    #[test]
    fn maps_a_fake_state_sequence() {
        let sequence = [
            State::Idle,
            State::Running,
            State::Offline,
            State::Running,
            State::Error(Arc::new(Error::Supervisor)),
            State::Running,
            State::Terminated,
        ];
        let mapped: Vec<SyncState> = sequence.iter().map(map_state).collect();
        assert_eq!(
            mapped,
            vec![
                SyncState::Idle,
                SyncState::Running,
                SyncState::Offline,
                SyncState::Running,
                SyncState::Error("sync failed".into()),
                SyncState::Running,
                SyncState::Terminated,
            ]
        );
    }
}
