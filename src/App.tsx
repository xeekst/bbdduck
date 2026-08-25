import { HashRouter, Navigate, Route, Routes } from "react-router-dom";
import { TooltipProvider } from "@/components/ui/tooltip";
import AppShell from "@/components/layout/AppShell";
import HomePage from "@/pages/HomePage";
import FileOccupancyPage from "@/pages/FileOccupancyPage";
import FileSyncPage from "@/pages/FileSyncPage";
import NetworkToolsPage from "@/pages/NetworkToolsPage";
import PortForwardPage from "@/pages/PortForwardPage";

function App() {
  return (
    <TooltipProvider>
      <HashRouter>
        <Routes>
          <Route element={<AppShell />}>
            <Route path="/" element={<HomePage />} />
            <Route path="/sync" element={<FileSyncPage />} />
            <Route path="/file-occupancy" element={<FileOccupancyPage />} />
            <Route path="/network" element={<NetworkToolsPage />} />
            <Route path="/port-forward" element={<PortForwardPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Route>
        </Routes>
      </HashRouter>
    </TooltipProvider>
  );
}

export default App;
