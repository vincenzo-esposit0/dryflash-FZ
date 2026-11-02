<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# dryflash GitHub Action

Runs a Flipper Zero firmware build in the dryflash emulator and reports on the pull request. It is a smoke test, not a hardware
certification: passing here does not guarantee it works on a device.

```yaml
on: pull_request
jobs:
  smoke:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write # to comment
    steps:
      - uses: actions/checkout@v7
      # ... build your firmware ...
      - uses: vincenzo-esposit0/dryflash-FZ/action@v1
        with:
          firmware: dist/f7-C/flipper-z-f7-update-local.tgz
```

| Input | Default | |
| --- | --- | --- |
| `firmware` | required | The build, `.tgz` or `.dfu` |
| `elf` | | Its ELF, to name functions in crash reports |
| `script` | the default smoke test | A YAML input script ([runner/README.md](../runner/README.md)) |
| `timeout` | `300` | Seconds of virtual time |
| `comment` | `true` | Comment on the pull request |
| `artifact-name` | `dryflash` | The artifact's name |
| `github-token` | `github.token` | Token to comment with |
| `package` | the matching GitHub Release | Which runner, as `npx --package` understands it |

Outputs: `result` (`pass`, `fail`, `emulator-limitation`), `exit-code` and `junit`. The job fails unless the exit code is 0, including when the
report blames the emulator. To let that through, set `continue-on-error` and check `result`.

The comment has the result, the crash report's verdict with its reasons, and the last screen as text. The API cannot attach images to a comment, so the
screenshots themselves are in the run's artifact, with the JUnit file and the crash report. Later runs replace the comment instead of adding more.

Pull requests from forks get a read-only token, so there the result goes only in the job summary.
