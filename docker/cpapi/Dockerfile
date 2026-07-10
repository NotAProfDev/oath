# Hand-rolled IBKR Client Portal Gateway (CP API v1). The gateway is a plain Java
# web server — no Xvfb/VNC/IBC (that machinery is only for the TWS desktop app).
FROM eclipse-temurin:21-jre

RUN apt-get update \
 && apt-get install -y --no-install-recommends unzip curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/clientportal

# Download + unpack the gateway distribution (extracts bin/, root/, dist/, ...).
ADD https://download2.interactivebrokers.com/portal/clientportal.gw.zip clientportal.gw.zip
RUN unzip -q clientportal.gw.zip && rm clientportal.gw.zip

EXPOSE 5000
# Uses the distribution's shipped root/conf.yaml (listens on :5000).
CMD ["bin/run.sh", "root/conf.yaml"]
