{{/*
Standard Helm helpers (bitnami-style). Centralized DSN resolution lives
in `sbol-db.databaseEnv`, which is the only template every workload
imports — keep all DSN-mode logic there.
*/}}

{{- define "sbol-db.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "sbol-db.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{- define "sbol-db.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "sbol-db.labels" -}}
helm.sh/chart: {{ include "sbol-db.chart" . }}
{{ include "sbol-db.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: sbol-db
{{- end -}}

{{- define "sbol-db.selectorLabels" -}}
app.kubernetes.io/name: {{ include "sbol-db.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "sbol-db.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "sbol-db.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{- define "sbol-db.image" -}}
{{- printf "%s:%s" .Values.image.repository (default .Chart.AppVersion .Values.image.tag) -}}
{{- end -}}

{{/*
Name of the Secret that holds the rendered DATABASE_URL. Only used when
the chart materializes the DSN itself (i.e. modes 2 and 3).
*/}}
{{- define "sbol-db.databaseSecretName" -}}
{{- printf "%s-db" (include "sbol-db.fullname" .) -}}
{{- end -}}

{{/*
Render the DATABASE_URL string for the chart-managed Secret. Fails fast
if neither `postgresql.enabled` nor `externalDatabase.url` is set.
Never call this when `externalDatabase.existingSecret.name` is set —
the existing Secret should be used directly without re-rendering.
*/}}
{{- define "sbol-db.databaseUrl" -}}
{{- if .Values.postgresql.enabled -}}
{{- $auth := .Values.postgresql.auth -}}
{{- $pw := required "postgresql.auth.password is required when postgresql.enabled=true" $auth.password -}}
{{- $host := printf "%s-postgresql" .Release.Name -}}
{{- printf "postgres://%s:%s@%s:5432/%s" $auth.username $pw $host $auth.database -}}
{{- else if .Values.externalDatabase.url -}}
{{- .Values.externalDatabase.url -}}
{{- else -}}
{{- fail "set externalDatabase.existingSecret.name, externalDatabase.url, or postgresql.enabled=true" -}}
{{- end -}}
{{- end -}}

{{/*
Emit the `env:` entry that resolves DATABASE_URL for the running pod.
Always reads from a Secret; never inlines the DSN into the manifest.
*/}}
{{- define "sbol-db.databaseEnv" -}}
- name: DATABASE_URL
  valueFrom:
    secretKeyRef:
{{- if .Values.externalDatabase.existingSecret.name }}
      name: {{ .Values.externalDatabase.existingSecret.name | quote }}
      key: {{ default "url" .Values.externalDatabase.existingSecret.key | quote }}
{{- else }}
      name: {{ include "sbol-db.databaseSecretName" . | quote }}
      key: url
{{- end }}
{{- end -}}

{{/*
Database-related env vars shared by every workload that talks to
Postgres (server Deployment + migrate Job). Pulls all knobs from
`.Values.config.database` and translates `0` into the env-var contract
the binary expects (`0` disables idle_timeout / max_lifetime).
*/}}
{{- define "sbol-db.databaseConfigEnv" -}}
{{- $db := .Values.config.database -}}
- name: DATABASE_MAX_CONNECTIONS
  value: {{ $db.maxConnections | quote }}
- name: DATABASE_MIN_CONNECTIONS
  value: {{ $db.minConnections | quote }}
- name: DATABASE_ACQUIRE_TIMEOUT_SECS
  value: {{ $db.acquireTimeoutSecs | quote }}
- name: DATABASE_IDLE_TIMEOUT_SECS
  value: {{ $db.idleTimeoutSecs | quote }}
- name: DATABASE_MAX_LIFETIME_SECS
  value: {{ $db.maxLifetimeSecs | quote }}
- name: DATABASE_CONNECT_TIMEOUT_SECS
  value: {{ $db.connectTimeoutSecs | quote }}
- name: DATABASE_STARTUP_TIMEOUT_SECS
  value: {{ $db.startupTimeoutSecs | quote }}
{{- end -}}

{{/*
Logging env vars. Shared by every workload — even the migrate Job
benefits from JSON output in container log aggregators.
*/}}
{{- define "sbol-db.loggingEnv" -}}
{{- if .Values.config.rustLog }}
- name: RUST_LOG
  value: {{ .Values.config.rustLog | quote }}
{{- end }}
{{- if and .Values.config.logFormat (ne .Values.config.logFormat "auto") }}
- name: LOG_FORMAT
  value: {{ .Values.config.logFormat | quote }}
{{- end }}
{{- end -}}

{{/*
Env vars specific to the long-running server process. Body limit + outer
request timeout are server-only; the migrate Job doesn't serve HTTP.
*/}}
{{- define "sbol-db.serveEnv" -}}
- name: SBOL_DB_BIND
  value: {{ .Values.config.bind | quote }}
- name: SBOL_DB_REQUEST_TIMEOUT_SECS
  value: {{ .Values.config.server.requestTimeoutSecs | quote }}
- name: SBOL_DB_MAX_BODY_BYTES
  value: {{ .Values.config.server.maxBodyBytes | quote }}
{{- end -}}
