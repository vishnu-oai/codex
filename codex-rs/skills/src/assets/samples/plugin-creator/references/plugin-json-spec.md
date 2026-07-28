# Plugin JSON sample spec

```json
{
  "name": "plugin-name",
  "version": "1.2.0",
  "description": "Brief plugin description",
  "author": {
    "name": "Author Name",
    "email": "author@example.com",
    "url": "https://github.com/author"
  },
  "homepage": "https://docs.example.com/plugin",
  "repository": "https://github.com/author/plugin",
  "license": "MIT",
  "keywords": ["keyword1", "keyword2"],
  "skills": "./skills/",
  "hooks": "./hooks.json",
  "mcpServers": "./.mcp.json",
  "apps": "./.app.json",
  "setup": {
    "inputs": [
      {
        "id": "project_root",
        "type": "directory",
        "prompt": "Where is your project directory?",
        "env": "CUSTOMER_PROJECT_ROOT",
        "required": true
      },
      {
        "id": "config_file",
        "type": "file",
        "prompt": "Where is your configuration file?",
        "env": "CUSTOMER_CONFIG_FILE",
        "required": true
      },
      {
        "id": "api_key",
        "type": "secret",
        "prompt": "Enter your MCP API key",
        "env": "CUSTOMER_MCP_API_KEY",
        "required": true
      }
    ],
    "commands": [
      {
        "name": "authenticate",
        "command": ["python3", "./scripts/authenticate.py"],
        "interactive": true
      },
      {
        "name": "write local configuration",
        "command": ["python3", "./scripts/configure.py"]
      },
      {
        "name": "verify MCP connection",
        "command": ["python3", "./scripts/verify.py"]
      }
    ]
  },
  "interface": {
    "displayName": "Plugin Display Name",
    "shortDescription": "Short description for subtitle",
    "longDescription": "Long description for details page",
    "developerName": "OpenAI",
    "category": "Productivity",
    "capabilities": ["Interactive", "Write"],
    "websiteURL": "https://openai.com/",
    "privacyPolicyURL": "https://openai.com/policies/row-privacy-policy/",
    "termsOfServiceURL": "https://openai.com/policies/row-terms-of-use/",
    "defaultPrompt": [
      "Summarize my inbox and draft replies for me.",
      "Find open bugs and turn them into Linear tickets.",
      "Review today's meetings and flag scheduling gaps."
    ],
    "brandColor": "#3B82F6",
    "composerIcon": "./assets/icon.png",
    "logo": "./assets/logo.png",
    "logoDark": "./assets/logo-dark.png",
    "screenshots": [
      "./assets/screenshot1.png",
      "./assets/screenshot2.png",
      "./assets/screenshot3.png"
    ]
  }
}
```

## Field guide

### Top-level fields

- `name` (`string`): Plugin identifier (kebab-case, no spaces). Required if `plugin.json` is provided and used as manifest name and component namespace.
- `version` (`string`): Plugin semantic version.
- `description` (`string`): Short purpose summary.
- `author` (`object`): Publisher identity.
  - `name` (`string`): Author or team name.
  - `email` (`string`): Contact email.
  - `url` (`string`): Author/team homepage or profile URL.
- `homepage` (`string`): Documentation URL for plugin usage.
- `repository` (`string`): Source code URL.
- `license` (`string`): License identifier (for example `MIT`, `Apache-2.0`).
- `keywords` (`array` of `string`): Search/discovery tags.
- `skills` (`string`): Relative path to skill directories/files.
- `hooks` (`string`): Hook config path.
- `mcpServers` (`string` or `object`): MCP config path, or an object whose keys are MCP server names and whose values are MCP server config objects.
- `apps` (`string`): App manifest path for plugin integrations.
- `setup` (`object`, optional): Opt-in, foreground CLI setup inputs and
  directly executed commands; see Experimental CLI plugin setup below.
- `interface` (`object`): Interface/UX metadata block for plugin presentation.

`mcpServers` may be declared as a companion file path:

```json
{
  "mcpServers": "./.mcp.json"
}
```

Or as an object directly in `plugin.json`:

```json
{
  "mcpServers": {
    "counter": {
      "type": "http",
      "url": "https://sample.example/counter/mcp"
    }
  }
}
```

### Experimental CLI plugin setup

Declare `setup` only when the plugin needs explicit first-install work, such as
generating an API key, collecting customer-specific paths, running an existing OAuth
login command, or writing local configuration. Setup is opt-in, disabled by
default, and currently available only in a foreground Unix CLI:

```bash
codex features enable plugin_setup
codex plugin add plugin-name@marketplace
```

Codex prints all declared commands, asks for approval, collects missing inputs
directly in the same terminal, and runs the steps in their declared order. It does
not open the Codex TUI or start an agent turn.

Each `setup.inputs` entry has:

- `id`: a unique ASCII input name used by `--set`.
- `type`: `text`, `directory`, `file`, or `secret`.
- `prompt`: the terminal question for a missing value.
- `env`: the unique environment variable passed to every setup step.
- `required`: whether a value is mandatory; the default is `true`.

Codex resolves file and directory inputs to existing absolute paths. Secret
values come from their declared environment variable or a hidden terminal
prompt; they are rejected in `--set` to keep them out of shell history and
process arguments.

Each `setup.commands` entry has:

- `name`: a unique human-readable step name.
- `command`: an executable-and-arguments array, never shell-evaluated.
- `interactive`: set `true` when the approved script needs the terminal
  for additional questions or authentication; the default is `false`.

Interactive commands require a real terminal for standard input, standard output,
and standard error. They receive the foreground terminal directly, so OAuth tools
and other authentication programs can prompt normally. Redirecting or piping any
of those streams is rejected. Their terminal output cannot be captured or
redacted; do not print credentials. Non-interactive command output is bounded,
sanitized, and redacted.

Every setup command inherits the invoking process environment and receives
`PLUGIN_ROOT`, `PLUGIN_DATA`, `CLAUDE_PLUGIN_ROOT`,
`CLAUDE_PLUGIN_DATA`, and the collected input environment variables.
Write generated files, tokens, and configuration under `PLUGIN_DATA` or
another explicitly selected location. `PLUGIN_ROOT` is the immutable
approved package; mutating it prevents setup from completing. Environment
changes made inside one command do not automatically persist to the parent
shell or the next command; use `PLUGIN_DATA` for state shared across steps.

For a non-interactive install, explicitly approve the plan, provide ordinary
inputs with `--set`, and put secrets in environment variables:

```bash
export CUSTOMER_MCP_API_KEY="your-api-key"

codex plugin add plugin-name@marketplace \
  --run-setup \
  --set project_root=/absolute/path/to/project \
  --set config_file=/absolute/path/to/config.yaml
```

An already-installed plugin can be configured again:

```bash
codex plugin setup plugin-name@marketplace

codex plugin setup plugin-name@marketplace \
  --yes \
  --set project_root=/absolute/path/to/project \
  --set config_file=/absolute/path/to/config.yaml
```

JSON output and unattended use require explicit approval and cannot run
`interactive` commands. A setup failure leaves a first installation inactive.
Background refreshes, marketplace updates, and desktop-app installs never run
setup. When a new plugin version changes its setup, remove and explicitly
reinstall the plugin so the new package and commands receive fresh approval.

### `interface` fields

- `displayName` (`string`): User-facing title shown for the plugin.
- `shortDescription` (`string`): Brief subtitle used in compact views.
- `longDescription` (`string`): Longer description used on details screens.
- `developerName` (`string`): Human-readable publisher name.
- `category` (`string`): Plugin category bucket.
- `capabilities` (`array` of `string`): Capability list from implementation.
- `websiteURL` (`string`): Public website for the plugin.
- `privacyPolicyURL` (`string`): Privacy policy URL.
- `termsOfServiceURL` (`string`): Terms of service URL.
- `defaultPrompt` (`array` of `string`): Starter prompts shown in composer/UX context.
  - Include at most 3 strings. Entries after the first 3 are ignored and will not be included.
  - Each string is capped at 128 characters. Longer entries are truncated.
  - Prefer short starter prompts around 50 characters so they scan well in the UI.
- `brandColor` (`string`): Theme color for the plugin card.
- `composerIcon` (`string`): Path to icon asset.
- `logo` (`string`): Path to logo asset.
- `logoDark` (`string`): Optional path to the logo asset used in dark mode.
- `screenshots` (`array` of `string`): List of screenshot asset paths.
  - Screenshot entries must be PNG filenames and stored under `./assets/`.
  - Keep file paths relative to plugin root.

### Path conventions and defaults

- Path values should be relative and begin with `./`.
- `skills`, `hooks`, and string-valued `mcpServers` are supplemented on top of default component discovery; they do not replace defaults.
- Custom path values must follow the plugin root convention and naming/namespacing rules.
- This repo’s scaffold writes `.codex-plugin/plugin.json`; treat that as the manifest location this skill generates.

# Marketplace JSON sample spec

`marketplace.json` depends on where the plugin should live. New plugin creation defaults to the
personal marketplace unless the caller explicitly requests a repo-local destination:

- Personal plugin: `~/.agents/plugins/marketplace.json`
- Repo/team plugin: `<repo-root>/.agents/plugins/marketplace.json`

```json
{
  "name": "openai-curated",
  "interface": {
    "displayName": "ChatGPT Official"
  },
  "plugins": [
    {
      "name": "linear",
      "source": {
        "source": "local",
        "path": "./plugins/linear"
      },
      "policy": {
        "installation": "AVAILABLE",
        "authentication": "ON_INSTALL"
      },
      "category": "Productivity"
    }
  ]
}
```

## Marketplace field guide

### Top-level fields

- `name` (`string`): Marketplace identifier or catalog name.
- `interface` (`object`, optional): Marketplace presentation metadata.
- `plugins` (`array`): Ordered plugin entries. This order determines how Codex renders plugins.

### `interface` fields

- `displayName` (`string`, optional): User-facing marketplace title.

### Plugin entry fields

- `name` (`string`): Plugin identifier. Match the plugin folder name and `plugin.json` `name`.
- `source` (`object`): Plugin source descriptor.
  - `source` (`string`): Use `local` for this repo workflow.
  - `path` (`string`): Relative plugin path based on the marketplace root.
    - Personal plugin in `~/.agents/plugins/marketplace.json`: `./plugins/<plugin-name>`
    - Repo/team plugin: `./plugins/<plugin-name>`
  - The same relative path convention is used for both personal and repo/team marketplaces.
    - Example: with `~/.agents/plugins/marketplace.json`, `./plugins/<plugin-name>` resolves to
      `~/plugins/<plugin-name>`.
- `policy` (`object`): Marketplace policy block. Always include it.
  - `installation` (`string`): Availability policy.
    - Allowed values: `NOT_AVAILABLE`, `AVAILABLE`, `INSTALLED_BY_DEFAULT`
    - Default for new entries: `AVAILABLE`
  - `authentication` (`string`): Authentication timing policy.
    - Allowed values: `ON_INSTALL`, `ON_USE`
    - Default for new entries: `ON_INSTALL`
  - `products` (`array` of `string`, optional): Product override for this plugin entry. Omit it unless product gating is explicitly requested.
- `category` (`string`): Display category bucket. Always include it.

### Marketplace generation rules

- `displayName` belongs under the top-level `interface` object, not individual plugin entries.
- When creating a new marketplace file from scratch, seed `interface.displayName` alongside top-level `name`.
- Always include `policy.installation`, `policy.authentication`, and `category` on every generated or updated plugin entry.
- Treat `policy.products` as an override and omit it unless explicitly requested.
- Append new entries unless the user explicitly requests reordering.
- Replace an existing entry for the same plugin only when overwrite is intentional.
- Default new plugin creation to the personal marketplace.
- Use a repo/team marketplace only when the user specifically requests that destination.
- Only override the marketplace `name` when the default `personal` name is already taken or
  installed and you need to seed a different new marketplace file.
- Choose marketplace location to match the selected destination:
  - Personal plugin: `~/.agents/plugins/marketplace.json`
  - Repo/team plugin: `<repo-root>/.agents/plugins/marketplace.json`

### Plugin validation notes

- The validator mirrors the workspace plugin ingestion schema so generated plugins follow the same
  manifest contract from the start.
- Plugin manifests must include real values for `name`, `version`, `description`,
  `author.name`, and the required `interface` fields.
- `version` must use strict semver.
- `websiteURL`, `privacyPolicyURL`, and `termsOfServiceURL` must be absolute `https://` URLs when
  present.
- `composerIcon`, `logo`, `logoDark`, and `screenshots` must point to real files inside the plugin archive when
  present.
- `apps` should appear in `plugin.json` only when `.app.json` actually exists.
- `mcpServers` may point to `.mcp.json` or contain the MCP server object directly in
  `plugin.json`.
- Validation rejects unsupported manifest fields such as `hooks`, so the scaffold keeps them out of
  generated manifests.
- Run `scripts/validate_plugin.py <plugin-path>` before handing back a generated plugin. It adds one
  intentional preflight check that rejects leftover `[TODO: ...]` placeholders.
