use super::*;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    id: uuid::Uuid,
    reference: String,
}
pub(super) fn get(state: &ServerState, request: &ApiRequest) -> ApiResult {
    let read: Read = params(request)?;
    if read.reference.len() != 64 || !read.reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiFailure::invalid("invalid attachment reference"));
    }
    let session = willdeep_core::SessionStore::new(&state.home)
        .load(read.id)
        .map_err(|_| ApiFailure::not_found("Session not found"))?;
    let message = session
        .messages
        .iter()
        .find(|message| {
            willdeep_core::conversation::attachment_reference(&message.attachments).as_deref()
                == Some(&read.reference)
        })
        .ok_or_else(|| ApiFailure::not_found("attachment source is no longer available"))?;
    json(serde_json::json!({"reference":read.reference,"attachments":message.attachments}))
}
