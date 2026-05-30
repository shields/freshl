<!--
Copyright © 2026 Michael Shields

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
-->

# Agents

Project conventions live in
[`shields/right-answers`](https://github.com/shields/right-answers) — follow
those by default.

## Filesystem races

`freshl` only reads and displays the filesystem — it never writes, changes
permissions, or performs a privileged check-then-act sequence, so it has no
security-sensitive TOCTOU surface. Don't guard against TOCTOU. If the filesystem
changes between when we stat an entry and when we render it, the listing simply
reflects a different point in time; that staleness is inherent to any listing
tool, so the resulting behavior is unspecified rather than a bug.

## Commit workflow

Before every commit:

1. Run `/code-review --fix` on the staged changes.
2. Submit through LGTMCP (`review_and_commit`).
