pub fn resolve_dev_server_port<F>(mut get_env: F) -> u16
where
    F: FnMut(&str) -> Option<String>,
{
    if let Some(port) = get_env("ROLLOUT_EDITOR_DEV_PORT")
        .and_then(|raw| raw.parse::<u16>().ok())
        .filter(|p| *p > 0)
    {
        return port;
    }

    if let Some(port) = get_env("VITE_PORT")
        .and_then(|raw| raw.parse::<u16>().ok())
        .filter(|p| *p > 0)
    {
        return port;
    }

    5173
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_dev_server_port_defaults_to_5173() {
        let port = resolve_dev_server_port(|_| None);
        assert_eq!(port, 5173);
    }

    #[test]
    fn resolve_dev_server_port_prefers_rollout_env() {
        let port = resolve_dev_server_port(|k| match k {
            "ROLLOUT_EDITOR_DEV_PORT" => Some("13491".to_string()),
            _ => None,
        });
        assert_eq!(port, 13491);
    }

    #[test]
    fn resolve_dev_server_port_falls_back_to_vite_port() {
        let port = resolve_dev_server_port(|k| match k {
            "VITE_PORT" => Some("15555".to_string()),
            _ => None,
        });
        assert_eq!(port, 15555);
    }

    #[test]
    fn resolve_dev_server_port_ignores_invalid_rollout_port_but_uses_valid_vite_port() {
        let port = resolve_dev_server_port(|k| match k {
            "ROLLOUT_EDITOR_DEV_PORT" => Some("not-a-port".to_string()),
            "VITE_PORT" => Some("15556".to_string()),
            _ => None,
        });
        assert_eq!(port, 15556);
    }

    #[test]
    fn resolve_dev_server_port_ignores_zero() {
        let port = resolve_dev_server_port(|k| match k {
            "ROLLOUT_EDITOR_DEV_PORT" => Some("0".to_string()),
            _ => None,
        });
        assert_eq!(port, 5173);
    }
}

