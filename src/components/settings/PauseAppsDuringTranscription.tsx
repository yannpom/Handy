import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useSettings } from "../../hooks/useSettings";
import { useOsType } from "../../hooks/useOsType";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { Input } from "../ui/Input";
import { Button } from "../ui/Button";
import { SettingContainer } from "../ui/SettingContainer";

interface PauseAppsDuringTranscriptionProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const PauseAppsDuringTranscription: React.FC<PauseAppsDuringTranscriptionProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const osType = useOsType();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const [newApp, setNewApp] = useState("");

    const enabled = getSetting("pause_apps_during_transcription") ?? false;
    const apps = getSetting("pause_apps_list") ?? [];

    // Pausing relies on SIGSTOP/SIGCONT, which Windows does not have.
    if (osType === "windows") {
      return null;
    }

    const handleAddApp = () => {
      const trimmed = newApp.trim();
      if (!trimmed || trimmed.length > 100) {
        return;
      }
      if (apps.some((app) => app.toLowerCase() === trimmed.toLowerCase())) {
        toast.error(
          t("settings.advanced.pauseApps.duplicate", { app: trimmed }),
        );
        return;
      }
      updateSetting("pause_apps_list", [...apps, trimmed]);
      setNewApp("");
    };

    const handleRemoveApp = (appToRemove: string) => {
      updateSetting(
        "pause_apps_list",
        apps.filter((app) => app !== appToRemove),
      );
    };

    const handleKeyDown = (e: React.KeyboardEvent) => {
      if (e.key === "Enter") {
        e.preventDefault();
        handleAddApp();
      }
    };

    return (
      <>
        <ToggleSwitch
          checked={enabled}
          onChange={(enabled) =>
            updateSetting("pause_apps_during_transcription", enabled)
          }
          isUpdating={isUpdating("pause_apps_during_transcription")}
          label={t("settings.advanced.pauseApps.label")}
          description={t("settings.advanced.pauseApps.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        {enabled && (
          <>
            <SettingContainer
              title={t("settings.advanced.pauseApps.appsTitle")}
              description={t("settings.advanced.pauseApps.appsDescription")}
              descriptionMode={descriptionMode}
              grouped={grouped}
            >
              <div className="flex items-center gap-2">
                <Input
                  type="text"
                  className="max-w-40"
                  value={newApp}
                  onChange={(e) => setNewApp(e.target.value)}
                  onKeyDown={handleKeyDown}
                  placeholder={t("settings.advanced.pauseApps.placeholder")}
                  variant="compact"
                  disabled={isUpdating("pause_apps_list")}
                />
                <Button
                  onClick={handleAddApp}
                  disabled={!newApp.trim() || isUpdating("pause_apps_list")}
                  variant="primary"
                  size="md"
                >
                  {t("settings.advanced.pauseApps.add")}
                </Button>
              </div>
            </SettingContainer>
            {apps.length > 0 && (
              <div
                className={`px-4 p-2 ${grouped ? "" : "rounded-lg border border-mid-gray/20"} flex flex-wrap gap-1`}
              >
                {apps.map((app) => (
                  <Button
                    key={app}
                    onClick={() => handleRemoveApp(app)}
                    disabled={isUpdating("pause_apps_list")}
                    variant="secondary"
                    size="sm"
                    className="inline-flex items-center gap-1 cursor-pointer"
                    aria-label={t("settings.advanced.pauseApps.remove", {
                      app,
                    })}
                  >
                    <span>{app}</span>
                    <svg
                      className="w-3 h-3"
                      fill="none"
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                    >
                      <path
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        strokeWidth={2}
                        d="M6 18L18 6M6 6l12 12"
                      />
                    </svg>
                  </Button>
                ))}
              </div>
            )}
          </>
        )}
      </>
    );
  });
