# Deployment YAML Reference

A run is invoked as `deploy-helper <deploy.yml>`. The deploy file defines what to run; the inventory file (`servers.yml` by default, or `-i <file>`) defines where to run it.

For CLI flags, extra-var input forms, and tag filtering, see [cli.md](cli.md).

## Inventory File

Maps host names used in deploy files to connection details.

```yaml
hosts:
  prod_web:
    host: 10.0.0.5
    port: 22
    user: deploy
    ssh_key_path: ~/.ssh/prod_web
  prod_db:
    host: db.internal
    user: deploy
    password: "{{ db_password }}"
  local:
    host: localhost
```

Fields per host:

- `host:` - IP or hostname. The literal value `localhost` runs commands locally instead of over SSH.
- `port:` - SSH port (default 22).
- `user:` - SSH user. Required for non-localhost.
- `password:` - SSH password. Prefer `ssh_key_path:` where possible.
- `ssh_key_path:` - path to the private key. Tilde-expanded.

`{{ var }}` placeholders in any of these are substituted from the current vars map.

## Deploy File Structure

A deploy file is a YAML stream of one or more deployments. Each deployment is a list with one entry. Use `---` to separate multiple deployments in one file.

```yaml
- name: Build
  hosts: prod_web
  tasks: [ ... ]

---

- name: Restart
  hosts: prod_web,prod_db
  tasks: [ ... ]
```

Deployment fields:

- `name:` - shown in the run banner.
- `hosts:` - comma-separated list of host names from the inventory. The deployment runs against each host in turn.
- `tasks:` - list of tasks (see below).
- `on_failure:` - optional flat task list run when a task in `tasks:` fails.
- `always:` - optional flat task list run after `tasks:` and any triggered `on_failure:`, whether they succeeded or failed.
- `vars:` - vars set before the deployment's tasks run.
- `chdir:` - default working directory for `shell:`, `command:`, `verify:`, and `env_file:` tasks. Tasks may override.
- `login_shell:` - if true, `shell:`, `command:`, and `verify:` run through a login shell (`$SHELL -l -i`) so `.bashrc`/`.zshrc` is loaded. Tasks may override.
- `shell_defaults:` - a line injected ahead of every `shell:` block, e.g. `set -u` or `set -euo pipefail`, so strict mode needn't be repeated per block. Runs but is not echoed, like the built-in `set -e`. Tasks may override; an empty string opts a task out.
- `environment:` - map of environment variables exported for every `shell:`, `command:`, and `verify:` task. Values are rendered through MiniJinja; keys must be plain identifiers. Never echoed, so secret values stay out of the output, and the exports ride inside the `become` wrapper so sudo/doas/su env resets don't strip them. Task-level entries merge over the deployment map per key.
- `become:` - if true, every task runs with privilege escalation by default. Tasks may override.
- `become_method:` - default elevation tool (`sudo`, `doas`, or `su`) for the deployment's tasks; applies where `become:` is in effect. Tasks may override.
- `tags:` - tags merged into every task's effective tag set. See [cli.md#tags](cli.md#tags).

## Recovery Tasks

Use play-level `on_failure:` for rollback and `always:` for cleanup. These are flat
task lists; nested recovery blocks are not supported.

```yaml
- name: Restore application data
  hosts: prod_web
  tasks:
    - name: Promote the restore candidate
      shell: |
        mv /srv/app.next /srv/app
        printf 'database_promoted=1\n'
      register: promotion_output

    - name: Verify the restored application
      verify:
        command: curl --fail --silent http://127.0.0.1/health

  on_failure:
    - name: Restore the previous database
      when: promotion_output is defined
      vars:
        promotion: "{{ promotion_output.stdout | from_env }}"
      environment:
        DATABASE_PROMOTED: "{{ promotion.database_promoted }}"
      shell: |
        test "$DATABASE_PROMOTED" = 1
        mv /srv/app.previous /srv/app

  always:
    - name: Remove the restore candidate
      shell: rm -rf /srv/app.next
```

`on_failure:` runs only after a returned task error. A successful `on_failure:`
section does not turn the deployment into a success; the original task error
remains the final error. If `on_failure:` or `always:` tasks also fail, their
errors are reported alongside the original error. `always:` runs even when
`on_failure:` fails.

Both sections inherit deployment vars, working directory, environment, shell
defaults, privilege escalation, and tags. Registered output from completed main
tasks remains available. Recovery tasks receive the special `always` tag, so a
positive `--tags` filter cannot silently exclude rollback or cleanup. Pass
`--skip-tags always` only when intentionally suppressing both sections.

`--list-tasks` prefixes potential recovery tasks with `[on_failure]` and `[always]`.

## Task Structure

Unknown keys are rejected everywhere - deployments, tasks, action specs, and inventory hosts - so a typo like `dst:` for `dest:` is a parse error naming the bad key instead of silently doing nothing.

Each task has a `name:` and one action key (`shell:`, `command:`, `template:`, `copy:`, `file:`, `env_file:`, `systemd:`, `verify:`, `debug:`, or `include_tasks:`). `debug:` is the one action that may be paired with another action on the same task; it runs first. Modifiers (`register:`, `when:`, `loop:`, `vars:`, `chdir:`, `login_shell:`, `become:`, `become_method:`, `tags:`) may be added to any task.

### `shell:`

Runs a block of shell code through `sh -c`. Multi-line blocks share state (variables, cwd, traps, shell options) because they execute as one shell invocation. An injected `set -e` stops the block on the first error.

```yaml
- name: Build and tag
  shell: |
    VERSION=$(git rev-parse --short HEAD)
    docker build -t app:$VERSION .
    docker tag app:$VERSION app:latest
```

Compound constructs (`if`, `case`, `for`, `while`, `until`, `select`) are kept as one segment for display; other lines are echoed individually before the block runs.

### `command:`

Runs each line as a standalone exec (no shell, no state shared between lines). Use this when you don't need shell features.

```yaml
- name: Restart services
  command: |
    systemctl restart nginx
    systemctl restart app
```

### `template:`

Renders a Jinja-style template file and writes it to a destination.

```yaml
- name: Write nginx config
  become: true
  template:
    src: templates/nginx.conf.j2
    dest: /etc/nginx/sites-available/{{ app_domain }}
```

`src:` is resolved relative to the deploy file's directory. The file's contents are rendered through MiniJinja using the current vars map. The `.j2` extension is convention only.

`mode:` sets the destination file's permissions. See [`mode:`](#mode) below.

### `copy:`

Writes a file from a static source or inline content, or copies a directory's contents recursively.

```yaml
- name: Static file copy
  copy:
    src: files/app.service
    dest: /etc/systemd/system/app.service

- name: Inline content
  copy:
    content: |
      APP_PORT={{ app_port }}
    dest: "{{ app_path }}/.env"

- name: Recursive directory copy
  copy:
    src: files/site
    dest: /var/www/site
```

Exactly one of `src:` or `content:` must be provided. `src:` is copied byte-for-byte without rendering. `content:` is rendered through MiniJinja.

When `src:` is a directory, its contents are copied recursively into `dest:` (like `cp -r src/. dest/`). Missing directories are created and matching files are overwritten, but unrelated files already in `dest:` are left untouched (nothing is deleted).

Symlinks inside `src:` are followed, not preserved: a link is copied as the file or directory it points to. A symlink that forms a cycle is not detected and will make the copy fail.

#### `mode:`

`copy:` and `template:` accept a `mode:` that sets the destination file's permissions:

```yaml
- name: Write credentials
  copy:
    content: |
      B2_KEY={{ b2_key }}
    dest: /etc/app/backup.env
    mode: "0600"
```

The value must be a string of octal digits (quote it: without a leading zero, YAML reads `600` as a number and the run fails with a hint). The file is staged next to `dest:` under a restrictive umask, chmod-ed, then atomically moved into place, so its content is never readable beyond the requested mode, not even between write and chmod. Not supported when `src:` is a directory.

### `file:`

Creates a directory (with parents, like `mkdir -p`) and applies permissions and ownership to it. Replaces `install -d -m ... -o ... -g ...` shell calls.

```yaml
- name: Create data directory
  become: true
  file:
    path: /srv/app/uploads
    state: directory
    mode: "0750"
    owner: "1000"
    group: "1000"
```

`state: directory` is required (no other states are supported yet). `mode:`, `owner:`, and `group:` are optional and apply to the final path component only; parents created along the way get default permissions. The task succeeds without changes if the directory already exists.

### `env_file:`

Builds one dotenv file from a defaults file, explicit values, and an optional
encrypted secrets file:

```yaml
- name: Materialize the application environment
  chdir: /opt/app
  env_file:
    defaults: .env.defaults
    values:
      APP_REF: "{{ app_ref }}"
    secrets:
      provider: sops
      src: secrets.enc.env
    dest: .env
    mode: "0600"
```

Paths are on the target and are relative to the effective `chdir:`. The
defaults file supplies the baseline. `values:` and decrypted secrets replace
matching defaults and add missing keys. They are peer overlays: defining a key
in both `values:` and secrets fails instead of choosing one silently.
`values:` are emitted as unquoted `KEY=value` entries and must use plain ASCII
scalars. Spaces, quotes, `#`, `$`, shell operators, and control characters are
rejected. Put complex values in the defaults or SOPS file so their original
dotenv quoting is preserved.

`provider: sops` runs `sops -d` on the target and redirects its output directly
into a restrictive temporary file. Decrypted content is never returned or
logged. When configured, decrypted secrets must contain at least one dotenv
entry. The final dotenv file contains each key once, uses LF line endings, and
is chmod-ed before an atomic move to `dest:`. A failure removes temporary files
and leaves an existing destination unchanged. `mode:` is required and follows
the same quoting and octal validation rules as `copy:` and `template:`.

### `systemd:`

Manages and verifies one or more systemd units:

```yaml
- name: Enable and verify application backups
  systemd:
    daemon_reload: true
    units:
      - name: pizen-app-backup.timer
        enabled: true
        state: started
        assert_active: true
      - name: pizen-app-backup.service
        state: started
        assert_result: success
```

`daemon_reload:` runs `systemctl daemon-reload` before processing units. Each
unit may set and verify `enabled: true` or `false`, request `state: started`, `stopped`,
`restarted`, or `reloaded`, verify without changing that it is enabled with
`assert_enabled: true`, verify that it is active with `assert_active: true`, and
compare its systemd `Result` property with `assert_result:`. Unit names and asserted
results support variable substitution. Units run in declaration order, and each
unit must request at least one operation or assertion. Duplicate and empty unit
names are rejected. `enabled: false` cannot be combined with `assert_enabled: true`,
and `state: stopped` cannot be combined with `assert_active: true`. Enabled checks
accept only systemd's `enabled` and `enabled-runtime` states. Missing units and
other inspection errors fail instead of being treated as disabled.

### `verify:`

Runs one shell command until it succeeds and its stdout matches an optional
expectation:

```yaml
- name: Wait for the application to become healthy
  verify:
    command: docker inspect --format '{{ "{{" }}.State.Health.Status{{ "}}" }}' app
    expect:
      equals: healthy
    retry:
      attempts: 12
      delay_seconds: 5
      max_elapsed_seconds: 60
```

`command:` runs as one shell invocation. A non-zero exit status always fails
the attempt. `expect:` may contain exactly one matcher:

- `equals:` compares the complete stdout exactly. The command runner removes
  trailing line endings but preserves all other whitespace.
- `regex:` succeeds when the Rust regular expression matches anywhere in
  stdout. Use anchors such as `^` and `$` when the whole output must match.

Without `expect:`, an exit status of zero is enough. Without `retry:`, the
command runs once. `retry.attempts` includes the first run and must be at least
1. `retry.delay_seconds` defaults to 0. `retry.max_elapsed_seconds` optionally
stops starting new attempts once that many seconds have elapsed and skips a
retry when its delay would reach the limit. An attempt already running is
allowed to finish. Commands, expected values, regexes, and
environment values support variable substitution. The final failure reports
the exit code or expected versus actual output. `no_log: true` hides the
command and failure details.

### `debug:`

Prints values from the current vars map. Useful for inspecting state mid-deployment.

```yaml
- name: Show resolved values
  debug:
    msg: "Deploying {{ app_name }} to {{ env }}"
```

### `include_tasks:`

Inlines tasks from another YAML file at this point in the deployment.

```yaml
- name: Run setup steps
  include_tasks: setup-steps.yml
```

The path is resolved relative to the deploy file's directory. Included tasks see the same vars map as the parent.

## Task Modifiers

These can be set on any task:

- `register: <name>` - capture the action's result (`stdout`, `stderr`, `rc`) into a var. `verify:` captures the final successful attempt. For `template:`, `copy:`, `file:`, `env_file:`, and `systemd:` the captured value is empty (`{stdout: "", stderr: "", rc: 0}`) since there is no command output.
- `no_log: true` - suppress this task's command echo and output (and `debug:` output) so secrets aren't printed. For `verify:`, it also hides failure details. `copy:`/`template:`/`file:`/`env_file:`/`systemd:` are unaffected since they never print their content. The `Executing task:` line still shows.
- `vars:` - set vars before the action runs. Available for substitution in the same task.
- `chdir: <path>` - working directory for `shell:`, `command:`, `verify:`, and `env_file:`. Falls back to the deployment-level `chdir:`.
- `when: <expr>` - skip the task unless the expression evaluates true.
- `creates: <path>` - skip the task if `<path>` already exists on the target (checked with `test -e`). Idempotency guard for `shell:`/`command:`.
- `removes: <path>` - skip the task if `<path>` does not exist on the target. Idempotency guard for `shell:`/`command:`.
- `loop: [...]` - run the action once per item; the current item is exposed as `{{ item }}`. List items may be scalars or maps (access fields as `{{ item.field }}`).
- `become: true` - run as root. `become_method:` selects the elevation tool (`sudo` default, `doas`, or `su`). Both fall back to the deployment-level `become:`/`become_method:`. See [cli.md#privilege-escalation-prompt](cli.md#privilege-escalation-prompt) for `become_password` handling.
- `login_shell: true` - run `shell:`, `command:`, and `verify:` through a login shell. Falls back to the deployment-level `login_shell:`.
- `shell_defaults: <line>` - override the deployment-level `shell_defaults:` for this task's `shell:` block. An empty string (`shell_defaults: ""`) disables the deployment default. Set on an `include_tasks:` task, the override applies to the included tasks (like `chdir:` and `login_shell:`).
- `environment:` - environment variables for this task's `shell:`/`command:`/`verify:`, merged over the deployment-level map (task entries win per key). Set on an `include_tasks:` task, the merged map applies to the included tasks.
- `tags: [...]` - task-level tags; merged with deployment and `include_tasks` tags into the task's effective tag set. See [cli.md#tags](cli.md#tags).

## Vars and Templating

Vars come from (later sources override earlier):

1. `--extra-vars` / `-e` on the CLI (repeatable). See [cli.md#extra-vars](cli.md#extra-vars) for input forms.
2. Deployment-level `vars:`.
3. Task-level `vars:`.
4. `register:` outputs from earlier tasks.

`{{ var }}` placeholders are rendered through MiniJinja in: task `name:`, deployment `name:`, deployment `chdir:`, task `chdir:`, all action bodies, and inventory `host:`/`user:`/`password:`/`ssh_key_path:`.

The `from_json` filter parses a JSON string into a value:

```yaml
- name: Parse stdout
  vars:
    parsed: "{{ json_output.stdout | from_json }}"
  debug:
    msg: "{{ parsed.Credentials.AccessKeyId }}"
```

The `from_env` filter parses simple `KEY=VALUE` output into a map:

```yaml
- name: Read a manifest
  command: print-manifest
  register: manifest_output

- name: Use the manifest
  vars:
    manifest: "{{ manifest_output.stdout | from_env }}"
  debug:
    msg: "{{ manifest.database_sha256 }}"
```

Keys must use letters, digits, and underscores, and cannot start with a digit.
Blank lines and comment lines beginning with `#` are ignored. Values remain
literal strings, may be empty, and may contain additional `=` characters.
Repeated keys and malformed lines are errors.
