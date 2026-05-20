use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RequestLogContext {
    pub request_id: String,
    pub route: &'static str,
    pub metadata_user_id: Option<String>,
    pub client_device_id: Option<String>,
    pub client_account_uuid: Option<String>,
    pub client_user: Option<String>,
    pub client_session_id: Option<String>,
    pub usage_session_key: String,
}

impl RequestLogContext {
    pub fn new_request_id() -> String {
        format!("msgreq_{}", short_uuid())
    }

    pub fn new(
        route: &'static str,
        metadata_user_id: Option<String>,
        usage_session_key: String,
    ) -> Self {
        Self::new_with_request_id(
            route,
            metadata_user_id,
            usage_session_key,
            Self::new_request_id(),
        )
    }

    pub fn new_with_request_id(
        route: &'static str,
        metadata_user_id: Option<String>,
        usage_session_key: String,
        request_id: String,
    ) -> Self {
        let parsed = metadata_user_id
            .as_deref()
            .and_then(|value| serde_json::from_str::<Value>(value).ok());

        Self {
            request_id,
            route,
            client_device_id: parsed
                .as_ref()
                .and_then(|value| json_string_field(value, "device_id")),
            client_account_uuid: parsed
                .as_ref()
                .and_then(|value| json_string_field(value, "account_uuid")),
            client_user: parsed
                .as_ref()
                .and_then(|value| json_string_field(value, "user")),
            client_session_id: parsed
                .as_ref()
                .and_then(|value| json_string_field(value, "session_id")),
            metadata_user_id,
            usage_session_key,
        }
    }

    pub fn metadata_user_id_for_log(&self) -> &str {
        self.metadata_user_id.as_deref().unwrap_or("")
    }

    pub fn metadata_user_id_present(&self) -> bool {
        self.metadata_user_id.is_some()
    }

    pub fn client_device_id_for_log(&self) -> &str {
        self.client_device_id.as_deref().unwrap_or("")
    }

    pub fn client_account_uuid_for_log(&self) -> &str {
        self.client_account_uuid.as_deref().unwrap_or("")
    }

    pub fn client_user_for_log(&self) -> &str {
        self.client_user.as_deref().unwrap_or("")
    }

    pub fn client_session_id_for_log(&self) -> &str {
        self.client_session_id.as_deref().unwrap_or("")
    }
}

fn json_string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

fn short_uuid() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..12].to_string()
}
