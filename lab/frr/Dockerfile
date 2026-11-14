# FRR with an SSH server, for the containerlab device suite: the FRR image
# has no SSH of its own. Both users log in with the password the container
# gets in NETTERM_LAB_PASSWORD (from the topology): root gets a shell, where
# `vtysh -c '...'` runs FRR commands; `vtysh` gets vtysh itself as its login
# shell, as a router's CLI (what the built-in `frr` platform drives).
#
#   docker build -t netterm-frr:latest lab/frr
#
FROM quay.io/frrouting/frr:10.2.1

RUN apk add --no-cache openssh-server \
 && sed -i -e 's/^#\?PermitRootLogin.*/PermitRootLogin yes/' \
           -e 's/^#\?PasswordAuthentication.*/PasswordAuthentication yes/' \
           /etc/ssh/sshd_config \
 && echo /usr/bin/vtysh >> /etc/shells \
 && adduser -D -s /usr/bin/vtysh vtysh \
 && addgroup vtysh frrvty

COPY start.sh /usr/local/bin/netterm-start
RUN chmod +x /usr/local/bin/netterm-start

ENTRYPOINT ["/sbin/tini", "--", "/usr/local/bin/netterm-start"]
