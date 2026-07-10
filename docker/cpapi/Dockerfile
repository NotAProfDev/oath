# Hand-rolled IBKR Client Portal Gateway (CP API v1). The gateway is a plain Java
# web server — no Xvfb/VNC/IBC (that machinery is only for the TWS desktop app).
FROM eclipse-temurin:21-jre

# `curl` is used by the container HEALTHCHECK below.
RUN apt-get update \
 && apt-get install -y --no-install-recommends unzip curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/clientportal

# Download + unpack the gateway distribution (extracts bin/, root/, dist/, ...).
# Unpinned: fetched over HTTPS from IBKR's official domain; IBKR ships no
# versioned/checksummed artifact, and this dev-only harness is never built in CI.
ADD https://download2.interactivebrokers.com/portal/clientportal.gw.zip clientportal.gw.zip
RUN unzip -q clientportal.gw.zip && rm clientportal.gw.zip

# Replace the distribution's shipped root/conf.yaml with our dev config: the shipped
# ips.allow (192.*/131.216.*/127.0.0.1) rejects a browser login forwarded through
# Docker (source 172.*/10.*) with 403. See conf.yaml.
COPY conf.yaml root/conf.yaml

# Run the Java gateway as a non-root user (Trivy DS-0002). Port 5000 is >1024, so
# an unprivileged user can bind it; the gateway writes logs/session under its dir,
# which we hand to `ibkr`.
RUN useradd -r -m -d /opt/clientportal -s /sbin/nologin ibkr \
 && chown -R ibkr:ibkr /opt/clientportal
USER ibkr

EXPOSE 5000

# The gateway answers /v1/api/iserver/auth/status with 200 (logged in) or 401 (not
# yet authenticated); both mean the JVM is up and serving. No -f, so a 401 is not a
# curl error; -k, since the gateway ships a self-signed cert.
HEALTHCHECK --interval=30s --timeout=5s --start-period=25s --retries=3 \
  CMD curl -sk --max-time 4 -o /dev/null -w '%{http_code}' \
      https://127.0.0.1:5000/v1/api/iserver/auth/status | grep -qE '^(200|401)$' || exit 1

# Uses the baked root/conf.yaml (listens on :5000).
CMD ["bin/run.sh", "root/conf.yaml"]
