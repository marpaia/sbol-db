# Importing iGEM parts

This guide assumes that:

- `/tmp/igem_parts.zip` is the iGEM archive inspected for this guide.
- You have a running SBOL DB instance, but do not want to import anything into
  your local development database while following the guide.

The archive contains 715 real SBOL 2 RDF/XML documents under
`igem_parts/out/`. It also contains 715 macOS metadata files under
`__MACOSX/`; those are not SBOL documents and must be ignored.

The real documents share collections, parts, and sequences. Import them all
into the same named graph with Graph Store `POST` requests. `POST` adds to the
graph, so rerunning the unchanged archive is safe. Do not change it to `PUT`,
which would replace the graph once per file.

## 1. Extract the SBOL documents

```bash
export IGEM_STAGE="$(mktemp -d /tmp/igem-parts.XXXXXX)"
unzip -q /tmp/igem_parts.zip 'igem_parts/out/*.xml' -d "$IGEM_STAGE"
export IGEM_PARTS_DIR="$IGEM_STAGE/igem_parts/out"

find "$IGEM_PARTS_DIR" -maxdepth 1 -type f -name '*.xml' | wc -l
```

The last command should print `715`. Extracting only `igem_parts/out/*.xml`
keeps the `__MACOSX` files out of the import.

## 2. Configure the destination

The SBOL DB server must have its authenticated Graph Store write endpoint
enabled. On the server, that means setting
`SBOL_DB_SPARQL_WRITE_ENABLED=true` and configuring
`SBOL_DB_SPARQL_AUTH_USER` and `SBOL_DB_SPARQL_AUTH_PASSWORD`. See
[SynBioHub compatibility](synbiohub.md#authentication) for the server-side
settings.

Set the destination values in the shell where you will run either client:

```bash
export SBOL_DB_BASE_URL="https://sbol.example.org"
export SBOL_DB_GRAPH="https://synbiohub.org/public"
export SBOL_DB_SPARQL_AUTH_USER="import-operator"
read -rsp 'SBOL DB password: ' SBOL_DB_SPARQL_AUTH_PASSWORD; echo
export SBOL_DB_SPARQL_AUTH_PASSWORD
```

Replace the example URL, username, and graph. The graph IRI is part of the RDF
identity; it must be the public graph used by your instance. Use HTTPS when
sending Basic-auth credentials.

## 3. Import with Python

Install the one dependency:

```bash
python3 -m pip install requests
```

Save this as `import_igem_parts.py`:

```python
from pathlib import Path
import os

import requests

parts_dir = Path(os.environ["IGEM_PARTS_DIR"])
base_url = os.environ["SBOL_DB_BASE_URL"].rstrip("/")
graph = os.environ["SBOL_DB_GRAPH"]
auth = (
    os.environ["SBOL_DB_SPARQL_AUTH_USER"],
    os.environ["SBOL_DB_SPARQL_AUTH_PASSWORD"],
)

files = sorted(parts_dir.glob("*.xml"))
if len(files) != 715:
    raise SystemExit(f"Expected 715 XML files in {parts_dir}, found {len(files)}")

endpoint = f"{base_url}/sparql-graph-crud-auth/"

for number, path in enumerate(files, start=1):
    response = requests.post(
        endpoint,
        params={"graph-uri": graph},
        auth=auth,
        headers={"Content-Type": "application/rdf+xml"},
        data=path.read_bytes(),
        timeout=120,
        allow_redirects=False,
    )

    if response.status_code != 200:
        raise RuntimeError(
            f"{path.name}: HTTP {response.status_code}: {response.text}"
        )

    inserted = response.json()["inserted"]
    print(f"[{number}/715] {path.name}: {inserted} new triples")

print("Imported all 715 iGEM documents.")
```

Run it:

```bash
python3 import_igem_parts.py
```

## 4. Import with Java 11+

This version uses only the Java standard library. Save it as
`ImportIgemParts.java`:

```java
import java.net.URI;
import java.net.URLEncoder;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Base64;
import java.util.Comparator;
import java.util.List;
import java.util.stream.Collectors;
import java.util.stream.Stream;

public class ImportIgemParts {
    private static String env(String name) {
        String value = System.getenv(name);
        if (value == null || value.isBlank()) {
            throw new IllegalArgumentException(name + " must be set");
        }
        return value;
    }

    public static void main(String[] args) throws Exception {
        Path partsDir = Path.of(env("IGEM_PARTS_DIR"));
        List<Path> files;
        try (Stream<Path> paths = Files.list(partsDir)) {
            files = paths
                    .filter(Files::isRegularFile)
                    .filter(path -> path.getFileName().toString().endsWith(".xml"))
                    .sorted(Comparator.comparing(path -> path.getFileName().toString()))
                    .collect(Collectors.toList());
        }

        if (files.size() != 715) {
            throw new IllegalStateException(
                    "Expected 715 XML files in " + partsDir + ", found " + files.size());
        }

        String baseUrl = env("SBOL_DB_BASE_URL").replaceAll("/+$", "");
        String graph = URLEncoder.encode(
                env("SBOL_DB_GRAPH"), StandardCharsets.UTF_8);
        URI endpoint = URI.create(
                baseUrl + "/sparql-graph-crud-auth/?graph-uri=" + graph);

        String userPassword = env("SBOL_DB_SPARQL_AUTH_USER") + ":"
                + env("SBOL_DB_SPARQL_AUTH_PASSWORD");
        String authorization = "Basic " + Base64.getEncoder().encodeToString(
                userPassword.getBytes(StandardCharsets.UTF_8));

        HttpClient client = HttpClient.newBuilder()
                .followRedirects(HttpClient.Redirect.NEVER)
                .build();

        for (int index = 0; index < files.size(); index++) {
            Path path = files.get(index);
            HttpRequest request = HttpRequest.newBuilder(endpoint)
                    .header("Authorization", authorization)
                    .header("Content-Type", "application/rdf+xml")
                    .POST(HttpRequest.BodyPublishers.ofFile(path))
                    .build();

            HttpResponse<String> response = client.send(
                    request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() != 200) {
                throw new RuntimeException(
                        path.getFileName() + ": HTTP " + response.statusCode()
                                + ": " + response.body());
            }

            System.out.printf("[%d/715] %s: %s%n",
                    index + 1, path.getFileName(), response.body());
        }

        System.out.println("Imported all 715 iGEM documents.");
    }
}
```

Compile and run it:

```bash
javac --release 11 ImportIgemParts.java
java ImportIgemParts
```

Run one client, not both. Both examples stop at the first failed request. Files
successfully sent before that point remain imported; fix the problem and rerun
the program to continue safely.

When finished, remove the password from the client shell:

```bash
unset SBOL_DB_SPARQL_AUTH_PASSWORD
```

This guide uses the Graph Store endpoint deliberately. SBOL DB's native
`POST /graphs` endpoint instead converts each upload into a separate SBOL 3
document, which is a different import model from preserving this overlapping
SBOL 2 corpus in one public graph.
