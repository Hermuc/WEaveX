import DetailsSidebar from "./details/DetailsSidebar";
import type { InstalledProps, WorkshopProps } from "@/types/details";

type Props = WorkshopProps | InstalledProps;

export default function DetailsPanel(props: Props) {
  return <DetailsSidebar {...props} />;
}
