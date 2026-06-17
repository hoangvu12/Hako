import { useParams } from "@tanstack/react-router";

import { ClipViewer } from "@/components/clips/clip-viewer";

export default function ClipDetailPage() {
  const { clipId } = useParams({ from: "/clips/$clipId" });
  return <ClipViewer clipId={clipId} />;
}
