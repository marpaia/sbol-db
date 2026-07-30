"""Native sbol-db semantic search using Hugging Face Transformers.

The model implementation is plain Python. sbol-db calls it for both document
maintenance and query embedding, then owns FAISS indexing, authorization
filters, result hydration, and the structured search response.
"""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any, Mapping, Optional

import torch
from transformers import AutoModel, AutoTokenizer

from sbol_db.search import EmbeddingKind, SearchContext, SearchPlugin

MODEL = "BAAI/bge-small-en-v1.5"
# Pin the model space: changing weights requires a new profile and index.
REVISION = "baab320e3049c6c62dd63560765566dd9083985e"
PROFILE = "python.huggingface.bge-small-en-v1.5.v1"
INDEX = "python.huggingface.bge-small-en-v1.5.v1"
STRATEGY = "python.huggingface.bge-small-search.v1"


class BgeSmallEmbedding:
    def __init__(self, *, device: Optional[str] = None) -> None:
        if device is not None:
            self.device = device
        elif torch.cuda.is_available():
            self.device = "cuda"
        elif torch.backends.mps.is_available():
            self.device = "mps"
        else:
            self.device = "cpu"
        self.tokenizer = AutoTokenizer.from_pretrained(MODEL, revision=REVISION)
        self.model = AutoModel.from_pretrained(MODEL, revision=REVISION).to(self.device)
        self.model.eval()

    @torch.inference_mode()
    def embed(self, texts: Sequence[str], *, kind: EmbeddingKind) -> list[list[float]]:
        # BGE v1.5 supports instruction-free retrieval, so query and document
        # inputs use the same text projection. `kind` remains explicit because
        # other models may require different query/document prefixes.
        del kind
        if not texts:
            return []
        encoded = self.tokenizer(
            list(texts),
            padding=True,
            truncation=True,
            max_length=512,
            return_tensors="pt",
        ).to(self.device)
        output = self.model(**encoded)
        vectors = torch.nn.functional.normalize(
            output.last_hidden_state[:, 0], p=2, dim=1
        )
        return vectors.cpu().tolist()


class BgeSmallSearch:
    def search(
        self, ctx: SearchContext, request: Mapping[str, Any]
    ) -> Mapping[str, Any]:
        query = request["query"]
        if query["kind"] != "text":
            raise ValueError("BGE-small search accepts text queries only")

        vector = ctx.embed(query["text"], kind="query")[0]
        graphs = request.get("filters", {}).get("graphs", [])
        graph_filter = (
            {"op": "any", "field": "graph", "values": list(graphs)}
            if graphs
            else None
        )
        requested_limit = int(request.get("page", {}).get("limit", 50))
        limit = min(requested_limit, int(ctx.budget["max_candidates"]))
        candidates = ctx.vectors.query(
            vector,
            filter=graph_filter,
            limit=limit,
            cursor=request.get("page", {}).get("cursor"),
        )

        candidate_items = candidates["items"]
        documents = ctx.documents.hydrate(
            [candidate["document_id"] for candidate in candidate_items]
        )
        documents_by_id = {document["document_id"]: document for document in documents}
        explain = bool(request.get("options", {}).get("explain", False))
        warnings = []
        items = []
        for rank, candidate in enumerate(candidate_items, start=1):
            document = documents_by_id.get(candidate["document_id"])
            if document is None:
                warnings.append(
                    f"authorized primary store did not hydrate {candidate['document_id']!r}"
                )
                continue
            hit = dict(document)
            hit.update(
                score=candidate["score"],
                score_kind="cosine_similarity",
                evidence=(
                    [
                        {
                            "source": STRATEGY,
                            "rank": rank,
                            "score": candidate["score"],
                            "details": {
                                "embedding_profile": PROFILE,
                                "vector_index": INDEX,
                                "vector_name": "content",
                            },
                        }
                    ]
                    if explain
                    else []
                ),
            )
            items.append(hit)

        return {
            "items": items,
            "next_cursor": candidates.get("next_cursor"),
            "execution": {"warnings": warnings},
        }


def register(search: SearchPlugin) -> None:
    embedding = BgeSmallEmbedding()
    search.add_embedding(
        embedding,
        id=PROFILE,
        provider="huggingface-transformers",
        model=MODEL,
        revision=REVISION,
        dimension=384,
        normalization="l2",
        data_egress="none",
    )
    search.add_strategy(
        BgeSmallSearch(),
        id=STRATEGY,
        version="1",
        display_name="Hugging Face BGE-small semantic search",
        description="Dense retrieval over canonical SBOL object metadata",
        embedding_profile=PROFILE,
        vector_index=INDEX,
        vector_name="content",
        distance="cosine",
    )
