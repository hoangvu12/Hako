import { useQuery } from "@tanstack/react-query";
import { getFfmpegInfo, getGpuInfo } from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";

/** GPU adapters + selected encoder. Static for a session, so cache forever. */
export function useGpuInfo() {
  return useQuery({
    queryKey: queryKeys.gpuInfo,
    queryFn: getGpuInfo,
    retry: false,
    staleTime: Infinity,
  });
}

/** Bundled FFmpeg versions + hardware encoder availability. */
export function useFfmpegInfo() {
  return useQuery({
    queryKey: queryKeys.ffmpegInfo,
    queryFn: getFfmpegInfo,
    retry: false,
    staleTime: Infinity,
  });
}
