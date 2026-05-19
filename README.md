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

# freshl

A modern replacement for `ls`.

## Behavior

`freshl` supports the greatest hits of `ls` options: `-R`, `-S`, `-t`, `-r`, and
`-d`.

Unlike `ls`, `-S` and `-t` put the largest/newest files at the bottom of the
output. Also unlike `ls`, you can use `-r` again to double-reverse: `-rt` will
output the oldest files last. `-rrrr` will have no effect but is amusingly
piratical.

`-R` ignores gitignored and dotfile dirs, unless with `-u` or `-uu` (à la
`ripgrep`).

Symlinks are always followed: a row reads `name → target` (or
`name → mid → target` for multi-hop), with the final target in its natural
per-kind color. Broken symlinks keep the arrow form with the target in red.

## Display

Color output is configured with `$LS_COLORS`, compatible with GNU `ls`. Apart
from that, there are _no display options_. You get it my way.

- Integrated Git status.
- File modes are in octal, e.g., `644` instead of `rw-r--r--`.
- Timestamps are always UTC.
- Directories sort first. Since symlinks-to-directories are followed, they sort
  with their targets.
- On case-insensitive filesystems, such as APFS, sorting is case-insensitive.

There is also context-sensitive dimming to deemphasize less-useful information:

- File modes are dimmed if they are the default for your current `umask`.
- Groups are dimmed if they are the primary group for that user.
- If a file size is 1 MB or more, the low-order digits are dimmed in groups of
  six. For example, a file of size 14142135 would have only `14` undimmed.
- A contiguous section of the timestamp is undimmed, generally three elements.
  Future timestamps are fully undimmed.

## Recommended usage

```bash
if which freshl >/dev/null; then
    alias l='freshl'
else
    alias l='ls -lA'
fi
```
