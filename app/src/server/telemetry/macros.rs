/// Discards a telemetry event: nothing is recorded or sent.
#[macro_export]
macro_rules! send_telemetry_sync_from_ctx {
    ($event:expr_2021, $ctx:expr_2021) => {
        let _ = $event;
        let _ = &$ctx;
    };
}

/// See [`send_telemetry_sync_from_ctx`].
#[macro_export]
macro_rules! send_telemetry_sync_from_app_ctx {
    ($event:expr_2021, $app_ctx:expr_2021) => {
        let _ = $event;
        let _ = &$app_ctx;
    };
}

/// See [`send_telemetry_sync_from_ctx`].
#[macro_export]
macro_rules! send_telemetry_on_executor {
    ($auth_state:expr_2021, $event:expr_2021, $executor:expr_2021) => {
        let _ = $event;
        let _ = (&$auth_state, &$executor);
    };
}
