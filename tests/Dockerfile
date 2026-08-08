FROM alpine:3.19

RUN apk add --no-cache openssh sudo doas util-linux-login

# Test-only SOPS stand-in. It verifies the provider invokes `sops -d <src>`
# and removes the fixture prefix without requiring real encryption keys.
RUN printf '%s\n' \
    '#!/bin/sh' \
    '[ "$#" -eq 2 ] && [ "$1" = "-d" ] || exit 64' \
    'if grep -q "^TEST_SOPS_FAIL$" "$2"; then exit 1; fi' \
    'if grep -q "^TEST_SOPS_EMPTY$" "$2"; then exit 0; fi' \
    'sed "s/^TEST_ENCRYPTED://" "$2"' \
    > /usr/local/bin/sops && chmod 755 /usr/local/bin/sops

# Test-only systemctl stand-in. Alpine does not run systemd, so record calls
# and return controlled unit states for the systemd module tests.
RUN printf '%s\n' \
    '#!/bin/sh' \
    'printf "%s\n" "$*" >> /tmp/deploy-helper-systemctl.log' \
    'case "$1" in' \
    '  daemon-reload|enable|disable|start|stop|restart|reload) exit 0 ;;' \
    '  is-active) case "$*" in *inactive.unit*) exit 3 ;; *) exit 0 ;; esac ;;' \
    '  is-enabled) case "$*" in' \
    '    *disabled.unit*) printf "disabled\n"; exit 1 ;;' \
    '    *static.unit*) printf "static\n"; exit 0 ;;' \
    '    *missing.unit*) printf "unit not found\n" >&2; exit 1 ;;' \
    '    *) printf "enabled\n"; exit 0 ;;' \
    '  esac ;;' \
    '  show) case "$*" in *failed.service*) printf "failed\n" ;; *) printf "success\n" ;; esac ;;' \
    '  *) exit 64 ;;' \
    'esac' \
    > /usr/local/bin/systemctl && chmod 755 /usr/local/bin/systemctl

RUN ssh-keygen -A

RUN echo "PasswordAuthentication yes" >> /etc/ssh/sshd_config && \
    echo "PermitRootLogin yes" >> /etc/ssh/sshd_config

RUN echo "root:password" | chpasswd

RUN adduser -D -s /bin/sh nopass && \
    echo "nopass:password" | chpasswd && \
    echo "Defaults !requiretty" >> /etc/sudoers && \
    echo "Defaults lecture=never" >> /etc/sudoers && \
    echo "nopass ALL=(ALL) NOPASSWD: ALL" >> /etc/sudoers

RUN adduser -D -s /bin/sh withpass && \
    echo "withpass:password" | chpasswd && \
    echo "withpass ALL=(ALL) ALL" >> /etc/sudoers

RUN adduser -D -s /bin/sh doasuser && \
    echo "doasuser:password" | chpasswd && \
    echo "permit nopass doasuser" > /etc/doas.conf && \
    adduser -D -s /bin/sh doaswithpass && \
    echo "doaswithpass:password" | chpasswd && \
    echo "permit doaswithpass" >> /etc/doas.conf && \
    chown root:root /etc/doas.conf && \
    chmod 400 /etc/doas.conf

EXPOSE 22

CMD ["/usr/sbin/sshd", "-D"]
