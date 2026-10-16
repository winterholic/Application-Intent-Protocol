FROM golang:alpine AS builder
RUN go install github.com/minio/minio@RELEASE.2025-09-06T17-38-46Z
RUN go install github.com/minio/mc@RELEASE.2025-08-13T08-35-41Z

FROM alpine:latest
RUN apk add --no-cache ca-certificates curl bash openssl
COPY --from=builder /go/bin/minio /usr/local/bin/minio
COPY --from=builder /go/bin/mc /usr/local/bin/mc
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
