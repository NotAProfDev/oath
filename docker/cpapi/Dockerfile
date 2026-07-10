# Hand-rolled IBKR Client Portal Gateway (CP API v1). The gateway is a plain Java
# web server — no Xvfb/VNC/IBC (that machinery is only for the TWS desktop app).
FROM eclipse-temurin:21-jre

RUN apt-get update \
 && apt-get install -y --no-install-recommends unzip ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/clientportal

# Download + unpack the gateway distribution (extracts bin/, root/, dist/, ...).
# Unpinned: fetched over HTTPS from IBKR's official domain; IBKR ships no
# versioned/checksummed artifact, and this dev-only harness is never built in CI.
ADD https://download2.interactivebrokers.com/portal/clientportal.gw.zip clientportal.gw.zip
RUN unzip -q clientportal.gw.zip && rm clientportal.gw.zip

# Run the Java gateway as a non-root user (Trivy DS-0002). Port 5000 is >1024, so
# an unprivileged user can bind it; the gateway writes logs/session under its dir,
# which we hand to `ibkr`.
RUN useradd -r -m -d /opt/clientportal -s /sbin/nologin ibkr \
 && chown -R ibkr:ibkr /opt/clientportal
USER ibkr

EXPOSE 5000
# Uses the distribution's shipped root/conf.yaml (listens on :5000).
CMD ["bin/run.sh", "root/conf.yaml"]
