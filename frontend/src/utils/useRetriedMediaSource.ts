import React from "react";
import {
  recordMediaLoad,
  scheduleMediaProbe,
} from "./mediaRetryCoordinator";

const RETRY_DELAYS_MS = [800, 1800, 3600, 7000];
const DEFAULT_MAX_CYCLES = 3;

type RetryState = {
  sourceIndex: number;
  attempt: number;
  cycle: number;
  loaded: boolean;
  retrying: boolean;
  failed: boolean;
};

const initialState: RetryState = {
  sourceIndex: 0,
  attempt: 0,
  cycle: 0,
  loaded: false,
  retrying: false,
  failed: false,
};

export function useRetriedMediaSource(
  sources: string[],
  options: { maxCycles?: number } = {},
) {
  const maxCycles = Math.max(1, options.maxCycles ?? DEFAULT_MAX_CYCLES);
  const signature = React.useMemo(
    () => `${maxCycles}\n${sources.join("\n")}`,
    [maxCycles, sources],
  );
  const lifecycleRef = React.useRef(0);
  const probeControllerRef = React.useRef<AbortController | null>(null);
  const signatureRef = React.useRef(signature);
  const [state, setState] = React.useState<RetryState>(initialState);

  React.useEffect(() => {
    if (signatureRef.current !== signature) {
      signatureRef.current = signature;
      lifecycleRef.current += 1;
      setState(initialState);
    }
    return () => {
      lifecycleRef.current += 1;
      probeControllerRef.current?.abort();
      probeControllerRef.current = null;
    };
  }, [signature]);

  const source = sources[state.sourceIndex] ?? null;

  const onLoad = React.useCallback(() => {
    recordMediaLoad(source);
    setState((current) => ({
      ...current,
      loaded: true,
      retrying: false,
      failed: false,
    }));
  }, [source]);

  const onError = React.useCallback(() => {
    if (state.failed) return;

    const baseRetryDelay = RETRY_DELAYS_MS[state.attempt];
    const retryDelay =
      baseRetryDelay === undefined
        ? undefined
        : Math.min(baseRetryDelay * 2 ** state.cycle, 30_000);
    if (retryDelay !== undefined) {
      const sourceIndex = state.sourceIndex;
      const attempt = state.attempt;
      const cycle = state.cycle;
      const lifecycle = lifecycleRef.current;
      const retrySource = sources[sourceIndex];
      if (!retrySource) {
        setState({ ...state, retrying: false, failed: true });
        return;
      }
      probeControllerRef.current?.abort();
      const controller = new AbortController();
      probeControllerRef.current = controller;
      void scheduleMediaProbe(retrySource, retryDelay, controller.signal).then(() => {
        if (lifecycleRef.current !== lifecycle) return;
        if (probeControllerRef.current === controller) {
          probeControllerRef.current = null;
        }
        setState((latest) => {
          if (
            latest.sourceIndex !== sourceIndex ||
            latest.attempt !== attempt ||
            latest.cycle !== cycle
          ) {
            return latest;
          }
          return {
            ...latest,
            attempt: attempt + 1,
            retrying: false,
          };
        });
      });
      setState({
        ...state,
        loaded: false,
        retrying: true,
      });
      return;
    }

    if (state.sourceIndex + 1 < sources.length) {
      setState({
        sourceIndex: state.sourceIndex + 1,
        attempt: 0,
        cycle: state.cycle,
        loaded: false,
        retrying: false,
        failed: false,
      });
      return;
    }

    if (state.cycle + 1 < maxCycles) {
      setState({
        sourceIndex: 0,
        attempt: 0,
        cycle: state.cycle + 1,
        loaded: false,
        retrying: false,
        failed: false,
      });
      return;
    }

    setState({
      ...state,
      loaded: false,
      retrying: false,
      failed: true,
    });
  }, [maxCycles, sources, state]);

  return {
    src: source,
    key: `${source ?? "empty"}:${state.cycle}:${state.sourceIndex}:${state.attempt}`,
    attempt: state.attempt,
    cycle: state.cycle,
    sourceIndex: state.sourceIndex,
    loaded: state.loaded,
    retrying: state.retrying,
    failed: state.failed || sources.length === 0,
    onLoad,
    onError,
  };
}
