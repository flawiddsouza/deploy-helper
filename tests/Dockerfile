FROM alpine:3.19

RUN apk add --no-cache openssh sudo doas

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
    chown root:root /etc/doas.conf && \
    chmod 400 /etc/doas.conf

EXPOSE 22

CMD ["/usr/sbin/sshd", "-D"]
