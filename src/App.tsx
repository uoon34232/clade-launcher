import { LauncherHome } from "@/components/launcher/LauncherHome";
import type { UiSettings } from "@/lib/settings/uiSettings";
import "./App.css";

export default function App({ initialUi }: { initialUi?: UiSettings }) {
  return <LauncherHome initialUi={initialUi} />;
}
