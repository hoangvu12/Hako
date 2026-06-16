import { useQuery } from "@tanstack/react-query";
import { getFfmpegInfo, getGpuInfo } from "@/lib/api";

/** GPU adapters + selected encoder. Static for a session, so cache forever. */
export function useGpuInfo() {
  return useQuery({
    queryKey: ["gpu-info"],
    queryFn: getGpuInfo,
    retry: false,
    staleTime: Infinity,
  });
}

/** Bundled FFmpeg versions + hardware encoder availability. */
export function useFfmpegInfo() {
  return useQuery({
    queryKey: ["ffmpeg-info"],
    queryFn: getFfmpegInfo,
    retry: false,
    staleTime: Infinity,
  });
}
