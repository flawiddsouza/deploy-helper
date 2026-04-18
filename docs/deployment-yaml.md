# Deployment YAML Reference

A run is invoked as `deploy-helper <deploy.yml>`. The deploy file defines what to run; the inventory file (`servers.yml` by default, or `-i <file>`) defines where to run it.

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
- `password:` - SSH password (use `ssh_key_path:` instead where possible).
- `ssh_key_path:` - path to the private key.

`{{ var }}` placeholders in any of these are substituted from the current vars map (extra vars + deployment vars). The path in `ssh_key_path:` is tilde-expanded.

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
- `vars:` - vars set before the deployment's tasks run.
- `chdir:` - default working directory for `shell:` and `command:` tasks. Tasks may override.
- `login_shell:` - if true, `shell:` and `command:` run through a login shell (`$SHELL -l -i`) so `.bashrc`/`.zshrc` is loaded. Tasks may override.

## Task Structure

Each task has a `name:` and exactly one action key (`shell:`, `command:`, `template:`, `copy:`, `debug:`, or `include_tasks:`). Modifiers like `register:`, `when:`, `loop:`, `become:`, `vars:`, `chdir:`, `login_shell:` may be added.

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

### `copy:`

Writes a file from a static source or inline content.

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
```

Exactly one of `src:` or `content:` must be provided. `src:` is copied byte-for-byte without rendering. `content:` is rendered through MiniJinja.

### `debug:`

Prints values from the current vars map. Useful for inspecting state mid-deployment.

```yaml
- name: Show resolved values
  debug:
    msg: "Deploying {{ app_name }} to {{ env }}"
```

`debug:` may be combined with another action on the same task; it runs first.

### `include_tasks:`

Inlines tasks from another YAML file at this point in the deployment.

```yaml
- name: Run setup steps
  include_tasks: setup-steps.yml
```

The path is resolved relative to the deploy file's directory. Included tasks see the same vars map as the parent.

## Task Modifiers

These can be set on any task:

- `register: <name>` - capture the action's result (`stdout`, `stderr`, `rc`) into a var. For `template:` and `copy:` the captured value is empty (`{stdout: "", stderr: "", rc: 0}`) since there is no command output.
- `vars:` - set vars before the action runs. Available for substitution in the same task.
- `chdir: <path>` - working directory for `shell:` and `command:`. Falls back to the deployment-level `chdir:`.
- `when: <expr>` - skip the task unless the expression evaluates true.
- `loop: [...]` - run the action once per item; the current item is exposed as `{{ item }}`. List items may be scalars or maps (access fields as `{{ item.field }}`).
- `become: true` - run as root. `become_method:` selects the elevation tool (`sudo` default, `doas`, or `su`). `become_password:` may be passed via extra vars; otherwise prompted interactively. `doas` is passwordless only.
- `login_shell: true` - run `shell:` and `command:` through a login shell. Falls back to the deployment-level `login_shell:`.

## Vars and Templating

Vars come from (later sources override earlier):

1. `--extra-vars` / `-e` on the CLI (repeatable).
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

## Extra Vars

`-e` accepts three forms (repeatable; later wins):

```sh
deploy-helper deploy.yml -e key=value -e 'a=1 b=2'
deploy-helper deploy.yml -e '{"key": "value", "n": 3}'
deploy-helper deploy.yml -e @vars.yml
```

- `key=value` (space-separated for multiple in one flag).
- A JSON object string (must start with `{`).
- `@<file>` to load a YAML file of vars.
