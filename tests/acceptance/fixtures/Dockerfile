FROM python:3.13.9-slim-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends openssl && rm -rf /var/lib/apt/lists/*
WORKDIR /fixture
COPY server.py .
USER 65532:65532
ENV FIXTURE_HOST=0.0.0.0 FIXTURE_PORT=8081
EXPOSE 8081
CMD ["python", "server.py"]
