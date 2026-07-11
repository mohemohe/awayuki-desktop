import { frontendHealthSnapshot, type FrontendHealthSnapshot } from "./observability";
import { invokeReadCommand } from "./tauri";

export type DiagnosticsSnapshot = {
  schemaVersion: number;
  activeOperations: number;
  completedOperations: number;
  failedOperations: number;
  apiRequests: number;
  httpRetries: number;
  rateLimitedErrors: number;
  dbTransactions: number;
  dbStatements: number;
  dbRows: number;
  dbQueryDurationMs: number;
  dbBusyErrors: number;
  cacheEntries: number;
  stream: {
    queueDepth: number;
    maxQueueDepth: number;
    coalesced: number;
    dropped: number;
    resyncs: number;
    resyncRequired: boolean;
  };
  droppedLogRecords: number;
  rollingEventCount: number;
};

export type SupportBundle = {
  schemaVersion: number;
  generatedAt: string;
  environment: {
    appVersion: string;
    databaseSchemaVersion: number;
    persistence: "sqlite_only_portable";
  };
  backend: DiagnosticsSnapshot;
  frontend: FrontendHealthSnapshot;
  recentEvents: Array<{
    at: string;
    operationId: string;
    command: string;
    phase: string;
    durationMs: number;
    resultCode: string;
    accountId?: string;
    metrics?: Record<string, number>;
  }>;
};

/// The support payload remains in memory. The user may inspect it, but this
/// helper never writes a diagnostic file or stores state outside SQLite.
export function createInMemorySupportBundle() {
  return invokeReadCommand<SupportBundle>("support_bundle", {
    request: { frontend: frontendHealthSnapshot() },
  });
}
