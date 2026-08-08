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
