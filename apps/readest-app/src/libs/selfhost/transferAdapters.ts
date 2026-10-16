// SELFHOSTED (fork): drop-in replacements for the utils/transfer primitives
// used by libs/storage.ts, keeping that shared file's call sites identical to
// upstream text. Two things a self-hosted server needs that the Readest cloud
// does not: (1) blob URLs may be path-only and must be absolutized against
// the runtime server base URL, and (2) path-only blob URLs (LocalDiskStore)
// need the Bearer token — pre-signed https URLs (R2/S3) carry their own
// credentials and must NOT get an Authorization header.

import { getAccessToken } from '@/utils/access';
import { getBaseUrl } from '@/services/environment';
import {
  tauriUpload as rawTauriUpload,
  tauriDownload as rawTauriDownload,
  webUpload as rawWebUpload,
  webDownload as rawWebDownload,
  type ProgressHandler,
} from '@/utils/transfer';

const toFullUrl = (url: string) => {
  if (/^https?:\/\//i.test(url)) return url;
  const base = getBaseUrl().replace(/\/+$/, '');
  if (url.startsWith('/')) return `${base}${url}`;
  return `${base}/${url}`;
};

const buildBlobAuthHeaders = async (url: string): Promise<Record<string, string>> => {
  if (/^https?:\/\//i.test(url)) return {};
  const token = await getAccessToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
};

export const webUpload = (
  file: File,
  uploadUrl: string,
  onProgress?: ProgressHandler,
): Promise<void> => {
  // The header must be computed per call (token freshness), so wrap in a
  // promise chain instead of an async signature change.
  return buildBlobAuthHeaders(uploadUrl).then((headers) =>
    rawWebUpload(file, toFullUrl(uploadUrl), onProgress, headers),
  );
};

export const tauriUpload = (
  url: string,
  filePath: string,
  method: Parameters<typeof rawTauriUpload>[2],
  onProgress?: ProgressHandler,
): Promise<string> => {
  return buildBlobAuthHeaders(url).then((headers) =>
    rawTauriUpload(toFullUrl(url), filePath, method, onProgress, headers),
  );
};

export const webDownload = (
  downloadUrl: string,
  onProgress?: ProgressHandler,
  headers?: Record<string, string>,
) => {
  return buildBlobAuthHeaders(downloadUrl).then((blobHeaders) =>
    rawWebDownload(toFullUrl(downloadUrl), onProgress, {
      ...blobHeaders,
      ...headers,
    }),
  );
};

export const tauriDownload = (
  url: string,
  filePath: string,
  onProgress?: ProgressHandler,
  headers?: Record<string, string>,
  body?: string,
  singleThreaded?: boolean,
  skipSslVerification?: boolean,
): Promise<Record<string, string>> => {
  return buildBlobAuthHeaders(url).then((blobHeaders) =>
    rawTauriDownload(
      toFullUrl(url),
      filePath,
      onProgress,
      { ...blobHeaders, ...headers },
      body,
      singleThreaded,
      skipSslVerification,
    ),
  );
};
