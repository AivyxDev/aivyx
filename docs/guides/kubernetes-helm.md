# Deploying Aivyx Engine on Kubernetes with Helm

This guide covers deploying Aivyx Engine to a Kubernetes cluster using the
official Helm chart.

## Prerequisites

- A Kubernetes cluster (v1.27+) with `kubectl` configured.
- Helm 3 installed.
- A container registry with the Aivyx Engine image (or access to the official
  registry).
- A bearer token and master key passphrase for Aivyx Engine configuration.

## Chart Location

The Helm chart is located in the Aivyx Engine repository:

```
deploy/helm/aivyx-engine/
  Chart.yaml
  values.yaml
  templates/
    deployment.yaml
    service.yaml
    ingress.yaml
    hpa.yaml
    configmap.yaml
    secret.yaml
    pvc.yaml
    serviceaccount.yaml
```

## Key Configuration Values

### Basic deployment

```yaml
# values.yaml
replicaCount: 1

image:
  repository: registry.example.com/aivyx/aivyx-engine
  tag: "0.7.4"
  pullPolicy: IfNotPresent

service:
  type: ClusterIP
  port: 3000

resources:
  requests:
    memory: "256Mi"
    cpu: "250m"
  limits:
    memory: "1Gi"
    cpu: "1000m"
```

### Persistent storage

Aivyx Engine stores encrypted data on disk via `redb`. Enable persistence to
survive pod restarts:

```yaml
persistence:
  enabled: true
  size: 10Gi
  storageClass: "standard"
  accessMode: ReadWriteOnce
  mountPath: /data
```

For production deployments, use a storage class with SSD-backed volumes for
better I/O performance.

### Autoscaling

Enable the Horizontal Pod Autoscaler for production workloads:

```yaml
autoscaling:
  enabled: true
  minReplicas: 2
  maxReplicas: 10
  targetCPUUtilizationPercentage: 70
  targetMemoryUtilizationPercentage: 80
```

Note: when running multiple replicas, ensure persistent storage uses
`ReadWriteMany` access mode or use a shared storage backend.

### Inline configuration

Provide `aivyx.toml` configuration inline:

```yaml
config: |
  [server]
  host = "0.0.0.0"
  port = 3000

  [auth]
  bearer_token_ref = "AIVYX_BEARER_TOKEN"

  [storage]
  path = "/data/aivyx.db"

  [billing]
  enabled = true

  [[billing.budgets]]
  scope = "global"
  daily_limit_usd = 100.00

  [[agents]]
  name = "default"
  model = "claude-sonnet-4-20250514"
  system_prompt = "You are a helpful assistant."
```

### Environment variables

Pass sensitive values and overrides via environment variables:

```yaml
env:
  - name: AIVYX_BEARER_TOKEN
    valueFrom:
      secretKeyRef:
        name: aivyx-secrets
        key: bearer-token
  - name: AIVYX_MASTER_PASSPHRASE
    valueFrom:
      secretKeyRef:
        name: aivyx-secrets
        key: master-passphrase
  - name: ANTHROPIC_API_KEY
    valueFrom:
      secretKeyRef:
        name: aivyx-secrets
        key: anthropic-api-key
```

## Secret Management

Create the Kubernetes secret before deploying:

```bash
kubectl create secret generic aivyx-secrets \
  --from-literal=bearer-token='your-secure-bearer-token' \
  --from-literal=master-passphrase='your-encryption-passphrase' \
  --from-literal=anthropic-api-key='sk-ant-...'
```

For production environments, consider using:

- **External Secrets Operator** to sync secrets from AWS Secrets Manager,
  HashiCorp Vault, or Azure Key Vault.
- **Sealed Secrets** for GitOps workflows where secrets are stored encrypted
  in version control.

## Ingress Configuration

### With TLS (recommended)

```yaml
ingress:
  enabled: true
  className: nginx
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
    nginx.ingress.kubernetes.io/proxy-body-size: "50m"
    nginx.ingress.kubernetes.io/proxy-read-timeout: "300"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "300"
  hosts:
    - host: aivyx.example.com
      paths:
        - path: /
          pathType: Prefix
  tls:
    - secretName: aivyx-tls
      hosts:
        - aivyx.example.com
```

### WebSocket support

The ingress must support WebSocket connections for the `/ws` and `/ws/voice`
endpoints. For nginx-ingress, add these annotations:

```yaml
nginx.ingress.kubernetes.io/proxy-http-version: "1.1"
nginx.ingress.kubernetes.io/proxy-set-headers: "Upgrade"
```

## Health Checks

The Helm chart configures liveness and readiness probes by default:

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 3000
  initialDelaySeconds: 10
  periodSeconds: 30
  failureThreshold: 3

readinessProbe:
  httpGet:
    path: /health
    port: 3000
  initialDelaySeconds: 5
  periodSeconds: 10
  failureThreshold: 3
```

The `/health` endpoint returns:

```json
{
  "status": "healthy",
  "version": "0.7.4",
  "uptime_seconds": 3600
}
```

## Deploying

### Install

```bash
helm install aivyx ./deploy/helm/aivyx-engine \
  -f custom-values.yaml \
  --namespace aivyx \
  --create-namespace
```

### Upgrade

```bash
helm upgrade aivyx ./deploy/helm/aivyx-engine \
  -f custom-values.yaml \
  --namespace aivyx
```

### Verify

```bash
kubectl get pods -n aivyx
kubectl logs -n aivyx deployment/aivyx-engine
```

Test the deployment:

```bash
kubectl port-forward -n aivyx svc/aivyx-engine 3000:3000

curl -s http://localhost:3000/health | jq .
curl -s -X POST http://localhost:3000/chat \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"agent": "default", "message": "Hello from Kubernetes!"}' | jq .
```

### Uninstall

```bash
helm uninstall aivyx --namespace aivyx
```

## Production Checklist

- [ ] TLS enabled via ingress with a valid certificate.
- [ ] Secrets managed via External Secrets Operator or equivalent.
- [ ] Persistent storage with SSD-backed storage class.
- [ ] HPA configured with appropriate min/max replicas.
- [ ] Resource requests and limits tuned for your workload.
- [ ] Network policies restricting pod-to-pod traffic.
- [ ] Monitoring and alerting configured (Prometheus + Grafana recommended).
- [ ] Log aggregation configured (Loki, Elasticsearch, or CloudWatch).
- [ ] Backup strategy for persistent volumes.
- [ ] Budget limits configured in `aivyx.toml` (see ADR-0005).
