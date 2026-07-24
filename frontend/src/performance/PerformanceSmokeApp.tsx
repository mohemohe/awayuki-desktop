import React from "react";

import {
  createTimelineEntityState,
  reduceTimelineEntities,
} from "../domain/timelineEntities";
import { TimelineStatusList } from "../features/timeline/TimelineStatusList";
import type { ColumnSummary, TimelineStatus } from "../types/app";
import {
  frontendRenderMetricsSnapshot,
  markNextRenderScenario,
  measureNextPaint,
  recordRenderDuration,
  recordReactCommit,
  resetFrontendRenderMetrics,
} from "../utils/renderMetrics";
import {
  frontendStartupMetricsSnapshot,
} from "../utils/startupMetrics";

const COLUMN_ID = "performance-smoke-timeline";
const INITIAL_STATUSES = 1_000;
const STREAM_EVENTS_PER_SECOND = 100;
const STREAM_SECONDS = 10;
const STREAM_BATCH_SIZE = 100;

console.info("AWAYUKI_PERFORMANCE_SMOKE module evaluated");

const column: ColumnSummary = {
  id: COLUMN_ID,
  columnType: "home",
  name: "Performance fixture",
  maxStatuses: INITIAL_STATUSES,
  paneIndex: 0,
  position: 0,
};

type Phase = "waiting" | "stream" | "scroll" | "profile" | "complete";

function initialEntityState() {
  return reduceTimelineEntities(createTimelineEntityState(), [
    {
      type: "replaceColumn",
      columnId: COLUMN_ID,
      statuses: Array.from({ length: INITIAL_STATUSES }, (_, index) =>
        fixtureStatus(index),
      ),
      limit: INITIAL_STATUSES,
    },
  ]);
}

export function PerformanceSmokeApp() {
  const [entities, setEntities] = React.useState(initialEntityState);
  const [phase, setPhase] = React.useState<Phase>("waiting");
  const [nearTop, setNearTop] = React.useState(true);
  const requestedAtRef = React.useRef<number | null>(null);
  const visibilityWaitMsRef = React.useRef(0);
  const streamCommitStartedAtRef = React.useRef<number | null>(null);
  const profileCommitStartedAtRef = React.useRef<number | null>(null);

  React.useEffect(() => {
    requestedAtRef.current = performance.now();
    console.info(
      "AWAYUKI_PERFORMANCE_SMOKE mounted",
      JSON.stringify({
        visibilityState: document.visibilityState,
        hidden: document.hidden,
        width: window.innerWidth,
        height: window.innerHeight,
      }),
    );
    const start = () => {
      if (document.hidden) return;
      visibilityWaitMsRef.current =
        performance.now() - (requestedAtRef.current ?? performance.now());
      resetFrontendRenderMetrics();
      setPhase("stream");
    };
    start();
    document.addEventListener("visibilitychange", start);
    return () => document.removeEventListener("visibilitychange", start);
  }, []);

  React.useLayoutEffect(() => {
    if (phase !== "stream" || streamCommitStartedAtRef.current === null) return;
    recordRenderDuration(
      "timeline:stream",
      performance.now() - streamCommitStartedAtRef.current,
    );
    streamCommitStartedAtRef.current = null;
  }, [entities, phase]);

  React.useLayoutEffect(() => {
    if (phase !== "profile" || profileCommitStartedAtRef.current === null) return;
    recordRenderDuration(
      "profile:open",
      performance.now() - profileCommitStartedAtRef.current,
    );
    profileCommitStartedAtRef.current = null;
  }, [phase]);

  React.useEffect(() => {
    if (phase !== "stream") return;
    let batch = 0;
    const totalBatches =
      (STREAM_EVENTS_PER_SECOND * STREAM_SECONDS) / STREAM_BATCH_SIZE;
    const timer = window.setInterval(() => {
      const first = INITIAL_STATUSES + batch * STREAM_BATCH_SIZE;
      const statuses = Array.from({ length: STREAM_BATCH_SIZE }, (_, index) =>
        fixtureStatus(first + index),
      );
      markNextRenderScenario("timeline:stream");
      measureNextPaint("timeline:stream");
      streamCommitStartedAtRef.current = performance.now();
      setEntities((current) =>
        reduceTimelineEntities(current, [
          {
            type: "mergeDelta",
            columnId: COLUMN_ID,
            statuses,
            limit: INITIAL_STATUSES,
          },
        ]),
      );
      batch += 1;
      if (batch >= totalBatches) {
        window.clearInterval(timer);
        window.setTimeout(() => setPhase("scroll"), 250);
      }
    }, 1_000);
    return () => window.clearInterval(timer);
  }, [phase]);

  React.useEffect(() => {
    if (phase !== "scroll") return;
    let frames = 0;
    const scroll = () => {
      const scroller = document.querySelector<HTMLElement>(
        '[data-virtuoso-scroller="true"]',
      );
      if (scroller) {
        scroller.scrollTop = frames % 2 === 0 ? 1_600 : 200;
        scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
      }
      markNextRenderScenario("timeline:scroll");
      measureNextPaint("timeline:scroll");
      frames += 1;
      if (frames < 5) requestAnimationFrame(scroll);
      else window.setTimeout(() => {
        profileCommitStartedAtRef.current = performance.now();
        setPhase("profile");
      }, 250);
    };
    requestAnimationFrame(scroll);
  }, [phase]);

  React.useEffect(() => {
    if (phase !== "profile") return;
    measureNextPaint("profile:open");
    const timer = window.setTimeout(() => setPhase("complete"), 750);
    return () => window.clearTimeout(timer);
  }, [phase]);

  React.useEffect(() => {
    if (phase !== "complete") return;
    const report = {
      schemaVersion: 1,
      fixtureId: "awayuki-webview-performance-v1",
      stream: {
        eventsPerSecond: STREAM_EVENTS_PER_SECOND,
        durationSeconds: STREAM_SECONDS,
        displayedStatuses: entities.timelines[COLUMN_ID]?.length ?? 0,
        visibilityWaitMs: Math.round(visibilityWaitMsRef.current),
      },
      startup: frontendStartupMetricsSnapshot(),
      render: frontendRenderMetricsSnapshot(),
      userAgent: navigator.userAgent,
    };
    console.info(`AWAYUKI_PERFORMANCE_REPORT ${JSON.stringify(report)}`);
  }, [entities.timelines, phase]);

  const statuses = entities.timelines[COLUMN_ID] ?? [];
  return (
    <main className="fixed inset-0 z-[9999] flex min-h-0 flex-col bg-base text-text">
      <header className="flex h-10 items-center justify-between border-b border-surface0 px-3 text-xs">
        <span>Awayuki WebView performance fixture</span>
        <span>{phase} · {nearTop ? "top" : "scrolled"}</span>
      </header>
      {phase === "waiting" ? (
        <div className="grid min-h-0 flex-1 place-items-center text-sm">
          Waiting for a visible WebView…
        </div>
      ) : phase === "profile" || phase === "complete" ? (
        <React.Profiler id="profile:open:smoke" onRender={recordReactCommit}>
          <section className="min-h-0 flex-1">
            <div className="border-b border-surface0 p-4">
              <div className="h-16 rounded bg-surface0" />
              <h1 className="mt-2 text-base">Synthetic profile</h1>
            </div>
            <TimelineStatusList
              column={column}
              statuses={statuses.slice(0, 80)}
              virtualized
              scrollTopRequest={0}
              isLoading={false}
              isLoadingMore={false}
              hasMore={false}
              onLoadMore={() => undefined}
              onNearTopChange={setNearTop}
              onScrollTopComplete={() => undefined}
            />
          </section>
        </React.Profiler>
      ) : (
        <React.Profiler id="timeline:smoke" onRender={recordReactCommit}>
          <TimelineStatusList
            column={column}
            statuses={statuses}
            virtualized
            scrollTopRequest={0}
            isLoading={false}
            isLoadingMore={false}
            hasMore={false}
            onLoadMore={() => undefined}
            onNearTopChange={setNearTop}
            onScrollTopComplete={() => undefined}
          />
        </React.Profiler>
      )}
    </main>
  );
}

function fixtureStatus(index: number): TimelineStatus {
  const id = `fixture-${index.toString().padStart(6, "0")}`;
  const createdAt = new Date(Date.UTC(2026, 0, 1) + index * 1_000).toISOString();
  return {
    id,
    originalStatusId: id,
    statusIdentity: {
      protocol: "activityPub",
      serverDomain: "benchmark.invalid",
      canonicalUri: `https://benchmark.invalid/statuses/${id}`,
      remoteId: id,
    },
    sourceAcct: "fixture@benchmark.invalid",
    accountId: `account-${index % 32}`,
    serverDomain: "benchmark.invalid",
    uri: `https://benchmark.invalid/statuses/${id}`,
    url: `https://benchmark.invalid/statuses/${id}`,
    displayName: `Fixture ${index % 32}`,
    acct: `fixture-${index % 32}@benchmark.invalid`,
    avatar: "",
    createdAt,
    content: `<p>Synthetic timeline row ${index}</p>`,
    spoilerText: "",
    reblogsCount: index % 17,
    favouritesCount: index % 31,
    repliesCount: index % 7,
    visibility: "public",
    sensitive: false,
    favourited: false,
    reblogged: false,
    bookmarked: false,
    media: [],
    emojis: [],
    accountEmojis: [],
  };
}
