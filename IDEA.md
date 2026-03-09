# MCPolly

## Summary

MCPolly is a status and observability enterprise-grade tool for AI agents. It is at its core an MCP server that AI agents can plug into to post their statuses as well as partial updates. It does not solely record finish statuses it records middle statuses as well. The AI tool plugs into the MCPolly server and sends over updates.

The user can view all of their agents in a unified web interface. They will eventually be able choose to receive SMS, Telegram, Slack or Discord bot updates. For the MVP lets focus on the interface first. They can view activity of the AI agent and also view reported errors as well. They can also configure alerts on MCPolly as well.

## Architecture

- Language: Rust
- Backend: Axum server for the API, Sqlite3 for the database for portability and efficiency
- Frontend: HTMX, simple UI elements, nothing that looks AI generated
- Core functionality: API Key secured MCP server that agents can easily plug into

- Production MVP hosting: RamNode on a $4 per month instance

## Development Plan

- Validate this tool locally
- We will need agents for: Product Planning, Product Design, Backend axum server API/MCP development, Frontend development with HTMX 

## MVP testing

- Test and validate locally, eventually deploy and test there