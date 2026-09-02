use crate::app_state::AppState;
use crate::domain::Message;
use crate::errors::AppError;

pub fn list_messages(
    state: &AppState,
    mailbox_id: Option<String>,
    limit: u32,
) -> Result<Vec<Message>, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .list_messages(mailbox_id.as_deref(), limit.min(500))
}
pub fn get_message(state: &AppState, message_id: String) -> Result<Message, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .get_message(&message_id)
}
pub fn mutate_message(
    state: &AppState,
    message_id: String,
    is_read: Option<bool>,
    is_starred: Option<bool>,
) -> Result<Message, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .mutate_message(&message_id, is_read, is_starred)
}

pub fn move_messages(
    state: &AppState,
    message_ids: Vec<String>,
    mailbox_id: String,
) -> Result<usize, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .move_messages(&message_ids, &mailbox_id)
}

pub fn delete_messages(
    state: &AppState,
    message_ids: Vec<String>,
    permanent: bool,
) -> Result<usize, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .delete_messages(&message_ids, permanent)
}
