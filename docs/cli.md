# CLI Reference

## Synopsis

```sh
deploy-helper <deploy_file> [options]
```

`<deploy_file>` is a path to a deployment YAML. See [deployment-yaml.md](deployment-yaml.md) for the file format, including the inventory schema.

## Options

| Flag | Purpose |
|------|---------|
| `-i`, `--inventory FILE` | Inventory YAML. Defaults to `servers.yml` in the current directory. |
| `-e`, `--extra-vars VARS` | Inject variables. Repeatable (later wins). See [Extra vars](#extra-vars). |
| `-t`, `--tags TAG[,TAG...]` | Run only tasks whose effective tags intersect this list. Repeatable. |
| `--skip-tags TAG[,TAG...]` | Exclude tasks whose effective tags intersect this list. Wins over `--tags`. |
| `--start-at-task NAME` | Skip tasks until one whose `name` matches exactly, then run from there. |
| `--step` | Prompt before each task: `(N)o/(y)es/(c)ontinue`. |
| `--list-tasks` | Print what would run under the current filters, then exit. |
| `-V`, `--version` | Print version and exit. |
| `-h`, `--help` | Print help and exit. |

## Extra vars

`-e` / `--extra-vars` accepts three forms and is repeatable. Later sources override earlier ones.

```sh
deploy-helper deploy.yml -e key=value
deploy-helper deploy.yml -e 'a=1 b=2'
deploy-helper deploy.yml -e '{"key": "value", "n": 3}'
deploy-helper deploy.yml -e @vars.yml
deploy-helper deploy.yml -e @defaults.yml -e @overrides.yml -e debug=true
```

- `key=value` sets one variable. Space-separate multiple in one flag (`'a=1 b=2'`).
- A JSON object string (must start with `{`) loads all keys at once.
- `@<file>` loads a YAML file of vars.

Extra vars sit at the bottom of the precedence stack. See [deployment-yaml.md#vars-and-templating](deployment-yaml.md#vars-and-templating) for the full merge order.

## Tags

Tasks, deployments, and `include_tasks:` entries may set `tags: [...]`. A task's effective tags are the union of its own tags, its deployment's tags, and the tags on any `include_tasks` that led to it. `--tags` keeps only matching tasks; `--skip-tags` drops matching tasks.

```yaml
- name: Web setup
  hosts: prod_web
  tags: [web]
  tasks:
    - name: Write nginx config
      tags: [nginx]
      template:
        src: templates/nginx.conf.j2
        dest: /etc/nginx/sites-available/app

    - name: Issue TLS cert
      tags: [tls]
      command: certbot certonly ...
```

`--tags nginx` runs only `Write nginx config` (effective tags `[web, nginx]`).

### Reserved tag names

- `always` runs the task regardless of `--tags`, unless `--skip-tags always` is set.
- `never` hides the task by default. Opt in with `--tags <name>`, either `never` itself or another of the task's tags.

```yaml
- name: Reload nginx
  tags: [always]
  command: systemctl reload nginx

- name: Wipe database
  tags: [never, destructive]
  command: psql -c "DROP DATABASE app"
```

`Reload nginx` runs on every invocation. `Wipe database` requires `--tags destructive` (or `--tags never`) to run.

### Filter order

For each task, gates apply in this order. The first matching gate decides:

1. **Start gate.** If `--start-at-task` is set and has not yet triggered, skip until a task whose name matches exactly. Once triggered, it stays triggered for the rest of the run.
2. **Always.** `always` tasks run, bypassing `--tags` and `--skip-tags`. The one exception is `--skip-tags always`.
3. **Never.** `never` tasks are skipped unless opted in via `--tags`.
4. **Tags filter.** If `--tags` was given, a task must share at least one tag with the filter.
5. **Skip-tags filter.** `--skip-tags` wins over `--tags` when both match.

## `--start-at-task`

Skips tasks until one whose `name:` matches exactly, then runs from there. Useful for resuming after a failure.

```sh
deploy-helper setup.yml --start-at-task "Issue TLS cert"
```

The match is against the rendered task name (after `{{ var }}` substitution). Once triggered, the gate stays open for the rest of the invocation, including subsequent deployments and included tasks.

## `--step`

Prompts before each task that would otherwise run. Tasks skipped by a filter or a `when:` condition do not prompt.

- `y` runs the task.
- `n` or empty (Enter) skips the task.
- `c` runs the current task and every subsequent task in the same deployment without further prompts. Prompting resumes on the next deployment.
- EOF on stdin is treated as skip.

`--step` is ignored when combined with `--list-tasks`.

## `--list-tasks`

Prints the task tree that would execute under the current filters, then exits without opening an SSH session.

```
$ deploy-helper setup.yml --list-tasks --tags web
Starting deployment: First-time setup
  Write nginx config  TAGS: [web, nginx]
  Issue TLS cert      TAGS: [web, tls]
```

`include_tasks` files are read and their children are shown indented. Filtered-out includes are not read. Task names are right-padded to the length of the longest visible name in each deployment.

## Privilege escalation prompt

Tasks with `become: true` elevate via `sudo` (default), `doas`, or `su`. If the chosen method needs a password:

1. Pass `become_password` via `-e`:
   ```sh
   deploy-helper deploy.yml -e become_password=$(cat ~/.secrets/sudo-pass)
   ```
2. Or leave it off and `deploy-helper` prompts once at the first `become:` task:
   ```
   BECOME password:
   ```
   The value is reused for every subsequent `become:` task in the same run.

`doas` is passwordless only. An empty `become_password=` is accepted; a non-empty value with `doas` is an error.

## Exit status

- `0` on success.
- Non-zero if any task fails, the YAML cannot be parsed, or an inventory host is missing. The failing task halts the deployment; subsequent deployments in the same file are not attempted.

## Examples

Run the full deploy against the default inventory:

```sh
deploy-helper setup.yml
```

Dry-list what `--tags web` would do:

```sh
deploy-helper setup.yml --list-tasks --tags web
```

Resume after a failure at the TLS step:

```sh
deploy-helper setup.yml --start-at-task "Issue TLS cert"
```

Step through interactively:

```sh
deploy-helper setup.yml --step
```

Inject a secret and target a staging inventory:

```sh
deploy-helper deploy.yml -i inventories/staging.yml -e @secrets.yml
```
