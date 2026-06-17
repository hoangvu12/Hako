import { createLazyRoute, useParams } from "@tanstack/react-router";

import { ClipViewer } from "@/components/clips/clip-viewer";

// Lazy-loaded: the trim editor / filmstrip code is deferred out of the boot
// bundle and fetched on navigation (preloaded on link intent).
export const Route = createLazyRoute("/clips/$clipId")({
  component: ClipDetailPage,
});

function ClipDetailPage() {
  const { clipId } = useParams({ from: "/clips/$clipId" });
  return <ClipViewer clipId={clipId} />;
}
