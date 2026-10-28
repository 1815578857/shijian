use crate::{models::ReasoningEffort, validation};

pub fn reasoning_options(base_url: &str, model: &str) -> Vec<ReasoningEffort> {
    use ReasoningEffort::*;
    let Ok(Some(url)) = validation::model_url(base_url) else {
        return vec![Default];
    };
    // Official DeepSeek Chat Completions contract; never infer from a model ID alone.
    // https://api-docs.deepseek.com/api/create-chat-completion/
    if url.scheme() == "https"
        && url.host_str() == Some("api.deepseek.com")
        && url.port_or_known_default() == Some(443)
        && matches!(url.path().trim_end_matches('/'), "" | "/v1")
        && matches!(model.trim(), "deepseek-flash" | "deepseek-v4-pro")
    {
        vec![Default, None, Low, High, Max]
    } else {
        vec![Default]
    }
}

pub fn effective_effort(base_url: &str, model: &str, effort: ReasoningEffort) -> ReasoningEffort {
    if reasoning_options(base_url, model).contains(&effort) {
        effort
    } else {
        ReasoningEffort::Default
    }
}

pub fn use_saved_key(draft: &str, saved: &str, requested: bool) -> Result<bool, String> {
    let draft = validation::normalized_model_base_url(draft)?;
    let saved = validation::normalized_model_base_url(saved)?;
    Ok(requested && !draft.is_empty() && draft == saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_only_official_endpoint_and_known_models() {
        use ReasoningEffort::*;
        for url in ["https://api.deepseek.com", "https://API.DEEPSEEK.COM:443/v1/"] {
            assert_eq!(reasoning_options(url, "deepseek-flash"), vec![Default, None, Low, High, Max]);
        }
        for url in ["https://api.deepseek.com.evil.test", "https://api.deepseek.com:8443", "https://api.deepseek.com/proxy", "http://localhost/v1", "https://api.deepseek.com?key=secret"] {
            assert_eq!(reasoning_options(url, "deepseek-flash"), vec![Default]);
        }
        assert_eq!(reasoning_options("https://api.deepseek.com", "unknown-model"), vec![Default]);
        assert_eq!(effective_effort("https://api.deepseek.com", "deepseek-flash", Medium), Default);
    }

    #[test]
    fn saved_key_requires_explicit_matching_full_address() {
        assert!(use_saved_key("https://API.EXAMPLE.COM:443/v1/", "https://api.example.com/v1", true).unwrap());
        assert!(!use_saved_key("https://api.example.com/v1", "https://api.example.com/v1", false).unwrap());
        for url in ["https://other.example/v1", "https://api.example.com/v2", "https://api.example.com/V1", "https://api.example.com:8443/v1"] {
            assert!(!use_saved_key(url, "https://api.example.com/v1", true).unwrap());
        }
    }
}
