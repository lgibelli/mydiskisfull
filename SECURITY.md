# Security

MyDiskIsFull reads a lot of file-system metadata and can delete files, so security and safety
bugs matter. A rule that deletes something it shouldn't is a security bug.

## Reporting

Please report vulnerabilities privately through GitHub: **Security → Report a vulnerability** on
this repository. Don't open a public issue.

Include what you ran, on which macOS version, and what happened. You'll get a reply within a few
days.

## Scope

- Anything that deletes, moves or modifies data the user didn't choose.
- Anything that causes network traffic (downloads or uploads) without being asked to.
- Privilege escalation through the planned helper.
- Exposure of file names or contents outside the machine. The app has no telemetry; any
  outbound data is a bug.
