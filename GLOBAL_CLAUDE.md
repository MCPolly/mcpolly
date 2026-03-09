## Agent Observability

At the start of every task:
1. Call `register_agent` with your agent name and a brief description.
2. Call `post_status` with state `starting` and a summary of the task.

As you work, call `post_status` at major milestones (state: `running`).

On completion, call `post_status` with state `completed`.

If you encounter an unrecoverable error, call `post_error` before stopping.
