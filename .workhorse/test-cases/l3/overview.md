# Doctor shows check details from the daemon's cached sweep

- [x] A cached sweep's warning, failing, broken, and skipped rows show the check's summary and its reason (verifies spec: DOC)
- [x] A cached row whose wire entry carries no reason prints no blank reason line
- [ ] On a host running alertd, a bare `bestool tamanu doctor` shows the same summaries and reasons as `--fresh`
