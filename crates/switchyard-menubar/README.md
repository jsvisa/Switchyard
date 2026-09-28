# switchyard-menubar

A macOS menu bar companion for a Switchyard server running in the background.

The server does the routing. This process only reads what the server already
wrote, so it can be quit and relaunched at any time without affecting traffic.

## What it shows

Click the glyph in the menu bar to see:

- whether the server is answering its health check
- today's and this week's requests and tokens
- what that traffic cost, against what it would have cost without routing
- which models served the week's tokens, ranked by share

It also offers "Restart server", which kicks the server's LaunchAgent, and
shortcuts to open the two config files.

## Install

From the repository root:

```sh
make install-macos
```

That builds `switchyard-server` and `switchyard-menubar`, installs them into
`~/.switchyard/bin`, writes a server config and menu bar settings, loads two
LaunchAgents, writes a `sy` Codex profile, and aliases `codex` to use it. Run
`make install-macos-dry-run` first to see every step without changing
anything, and `make uninstall-macos` to undo it.

Your server config and menu bar settings are never overwritten once they
exist, so your edits survive a reinstall. The Codex profile is rewritten each
time, keeping the previous version as a timestamped backup.

## The Codex profile

`codex --profile sy` reads `~/.codex/sy.config.toml`. A `[profiles.sy]` table
inside `config.toml` is legacy config that Codex refuses to start with, so the
installer removes one if an older version of this script left it there.

The profile sets only the model and the provider:

```toml
model = "switchyard"
model_provider = "sy"

[model_providers.sy]
name = "Switchyard"
base_url = "http://127.0.0.1:4123/v1"
wire_api = "responses"
requires_openai_auth = true
```

It deliberately does not set `approval_policy` or `sandbox_mode`. Routing
through Switchyard should not quietly change how Codex asks before it acts.
Set those in your own config if you want them.

## Codex.app

Codex.app cannot use the `sy` profile. It writes profiles as `[profiles.NAME]`
tables inside `config.toml`, the format its own bundled CLI now rejects as
legacy config. The two profile systems are mid-migration and conflict.

So the app gets a whole config instead of a profile. The installer builds
`~/.codex/config.toml.sy`: your `config.toml` with the routing inlined at the
top level, which is the one format both the app and the CLI accept. Everything
else in your config is carried over untouched.

Swap it in, then restart Codex.app:

```sh
cp ~/.codex/config.toml.sy ~/.codex/config.toml       # route through Switchyard
cp ~/.codex/config.toml.direct ~/.codex/config.toml   # back to normal
```

`config.toml.direct` is a snapshot of your unrouted config, taken at install
time. The installer never takes it while `config.toml` is already routed, so a
reinstall cannot clobber it, and `make uninstall-macos` puts it back before it
removes the server.

`config.toml.sy` is rebuilt on every install from whatever `config.toml` holds
at the time. Edit `config.toml`, not the generated file.

Note that swapping routes **everything** the app does through Switchyard.
Unlike the CLI alias, there is no unrouted escape hatch while it is swapped in.

## Settings

`~/.switchyard/menubar.toml`:

```toml
server_url = "http://127.0.0.1:4123"
routing_log = "~/.switchyard/routing.jsonl"
config_file = "~/.switchyard/composite.toml"
launchd_label = "com.nvidia.switchyard.server"
refresh_seconds = 30
baseline_model = "gpt-5.6-sol"

[prices."gpt-5.6-sol"]
input_per_mtok = 1.25
cached_input_per_mtok = 0.125
output_per_mtok = 10.0
```

Dollar figures are hidden until every model seen in the log has a price entry,
so a partial table never produces a misleading number.

## How savings are computed

The server writes one JSONL record per call to `~/.switchyard/routing.jsonl`,
including the model that answered and its token counts. Records are folded
into local-date buckets.

- **Actual** is every call priced at the model that served it, including
  Switchyard's own classifier calls.
- **Baseline** is the caller-facing calls only, priced as if every one of them
  had used `baseline_model`. Classifier calls have no baseline counterpart,
  because without Switchyard they would not happen.

Savings are the difference. Routing overhead therefore counts against the
figure, and a day where classification cost more than the cheaper tier saved
will show a negative number.

These are list-price estimates. On a ChatGPT login there is no per-token bill,
so treat the figure as "what this traffic would have cost at API rates".

## Logs

`~/.switchyard/logs/` holds stdout and stderr for both LaunchAgents.
