# flanforge-utils

Pure leaf helpers at the bottom of the workspace dependency graph.

- Bounds UTF-8 text to a byte budget without splitting a character, and reads
  the Unix clock in both a lenient and a fallible form.
- Matches allowlist patterns: `*` and `?` only, anchored at both ends, with a
  single backtrack point so repeated wildcards cannot cost exponential time.
- Checks Git refs, and ref allowlist patterns, against one conservative
  command-safe shape that always requires a `refs/` prefix.
- Decides which libvirt connection URIs FlanForge will drive: clear-text
  `qemu+tcp` only on an explicit opt-in, and never a URI with a query string.
- Has no dependencies at all, workspace or third-party, which is what lets
  `flanforge-core` and `flanforge-libvirt-wire` share the same predicates
  without depending on each other.
