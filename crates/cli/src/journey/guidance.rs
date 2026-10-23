//! Browser steps for the account-owned parts of onboarding; never account automation.
//! Sources: docs/ONBOARDING.md (official OpenAI setup and permissions guides).

pub(super) fn tunnel_setup() {
    eprintln!("Open https://platform.openai.com/settings/organization/tunnels");
    eprintln!(
        "Choose your Platform organization (your personal organization for a personal account)."
    );
    eprintln!("Open an existing tunnel, or create one, and associate it with the ChatGPT workspace where you will use the TV plugin.");
    eprintln!(
        "Copy its complete tunnel_... ID below. Type help for permissions, or q to resume later."
    );
}

pub(super) fn permissions() {
    eprintln!("Tunnel access belongs to the organization: Read + Manage to create; Read + Use for the runtime key owner and plugin operator.");
    eprintln!("If access is blocked, ask that organization's owner or RBAC administrator. Role changes can take up to 30 minutes.");
    eprintln!("Roles: https://platform.openai.com/settings/organization/people/roles");
    eprintln!("Groups: https://platform.openai.com/settings/organization/people/groups");
    eprintln!("Your ChatGPT workspace must also allow custom MCP plugins. This installer cannot grant account access.");
}

pub(super) fn runtime_key() {
    eprintln!(
        "Open https://platform.openai.com/settings/organization/api-keys in the same organization."
    );
    eprintln!("Create a Restricted runtime key with Tunnels Read + Use. Its owner also needs those permissions; do not use All or an admin key.");
    eprintln!("Paste the key only into the hidden prompt below, never into chat, screenshots or shell commands. tv-control does not save it; q resumes later.");
}

pub(super) fn plugin(tunnel_id: &str) {
    eprintln!(
        "Open https://chatgpt.com/plugins in your browser and select the intended workspace."
    );
    eprintln!("Use + → Add custom MCP server; name it tv-control. Under Connection choose Tunnel, then select or paste {tunnel_id}.");
    eprintln!("For this local stdio server choose No authentication; access is still controlled by the OpenAI tunnel. Do not paste the runtime key into ChatGPT.");
    eprintln!("Review the risk warning yourself, then Create as a plugin and install it. In your intended conversation, type @ and select tv-control.");
    eprintln!("If unavailable, check workspace permissions/security restrictions. Keep this controller running for discovery and calls.");
}
