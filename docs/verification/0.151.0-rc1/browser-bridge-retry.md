# Browser bridge 0.151.0-rc1

- Result: passed
- Fixture: http://127.0.0.1:19851
- Started: 2026-11-16T23:44:41Z

| Test | Result | Detail |
| --- | --- | --- |
| isolated_profile_clean_environment_and_dependencies | passed | {"node_version":"24.8.0","chrome_version":"Chrome/157.0.8103.79","profile_isolated":true,"clean_environment":true} |
| open_and_snapshot_local_form | passed | {"epoch":4,"node_count":7,"http_status":200} |
| fill_submit_and_business_assertion | passed | {"form":"submitted once","assertion":"status text equals Saved","passed":true} |
| screenshot_png_hash_mime_and_dimensions | passed | {"mime":"image/png","width":1280,"height":720,"bytes":21870,"sha256":"d35971e4c67f7ca6279e0dcad7f9771ebc49eac0de1f006019f83227ce4dc923","persistence":"protocol bytes only; Rust host owns file path"} |
| route_navigation_and_stale_epoch_refusal | passed | {"url":"http://127.0.0.1:19851/next","stale_epoch_code":"STALE_PAGE"} |
| origin_denial_and_no_arbitrary_actions | passed | {"origin_code":"ORIGIN_DENIED","arbitrary_action_code":"INVALID_ARGUMENTS"} |
| page_world_dom_spoofing_cannot_redirect_approved_click | passed | {"result":"ELEMENT_DENIED","target_and_overlay":"untouched","controller_world":"isolated"} |
| synthetic_application_error_state | passed | {"http_status":200,"application_status":"Synthetic error","assertion_passed":true} |
| close_removes_owned_profile | passed | {"closed":true,"profile_removed":true} |
| driver_cancellation_kills_isolated_node_chrome_process_group | passed | {"owned_group_members_before":12,"executing_members_after":0,"cancellation":"driver process-group kill; Rust future cancellation is covered separately by native tests"} |
| online_status_probes_hung_chrome_and_fails_within_deadline | passed | {"live_probe":true,"hung_status":"UNKNOWN","seconds":1.003,"automatic_retry":false} |

## Boundaries

- Real Node/Chrome CDP bridge; Rust host and shared execute/approval not compiled or executed by this script.
- Navigation/request origin controls and Chrome background flags; not whole-process network isolation.
- One disposable profile per process, clean environment; no dependency installation or external service.

## Not verified

- Rust compilation/unit tests
- Runtime approval and host ref binding
- Windows process job
- OS-level network isolation
