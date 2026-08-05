export class HttpError extends Error {
  readonly status: number;
  readonly body: string;
  readonly code?: string;

  constructor({
    status,
    message,
    body = "",
    code,
  }: {
    status: number;
    message: string;
    body?: string;
    code?: string;
  }) {
    super(message);
    this.status = status;
    this.body = body;
    this.code = code;
  }
}

export interface StructuredErrorBody {
  message?: string;
  code?: string;
}

export type ResponseErrorFactory = (response: Response) => Promise<Error>;

export async function requestJson<T>(
  input: RequestInfo | URL,
  init?: RequestInit,
  errorFactory: ResponseErrorFactory = defaultResponseError
): Promise<T> {
  const response = await fetch(input, init);
  if (!response.ok) throw await errorFactory(response);
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}

export async function requestEmpty(
  input: RequestInfo | URL,
  init?: RequestInit,
  errorFactory: ResponseErrorFactory = defaultResponseError
): Promise<void> {
  const response = await fetch(input, init);
  if (!response.ok) throw await errorFactory(response);
}

export async function requestText(
  input: RequestInfo | URL,
  init?: RequestInit,
  errorFactory: ResponseErrorFactory = defaultResponseError
): Promise<string> {
  const response = await fetch(input, init);
  if (!response.ok) throw await errorFactory(response);
  return response.text();
}

export function parseStructuredErrorBody(
  body: string
): StructuredErrorBody | null {
  try {
    const parsed = JSON.parse(body) as {
      error?: { code?: string; message?: string };
      detail?: string;
    };
    return {
      message: parsed.error?.message ?? parsed.detail,
      code: parsed.error?.code,
    };
  } catch {
    return null;
  }
}

export async function defaultResponseError(
  response: Response
): Promise<HttpError> {
  const body = await response.text().catch(() => "");
  const structured = parseStructuredErrorBody(body);
  return new HttpError({
    status: response.status,
    message:
      structured?.message ||
      body ||
      `HTTP ${response.status}: ${response.statusText}`,
    body,
    code: structured?.code,
  });
}
