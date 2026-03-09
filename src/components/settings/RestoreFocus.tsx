import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface RestoreFocusProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const RestoreFocus: React.FC<RestoreFocusProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("restore_focus_before_paste") ?? true;

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(enabled) =>
          updateSetting("restore_focus_before_paste", enabled)
        }
        isUpdating={isUpdating("restore_focus_before_paste")}
        label={t("settings.advanced.restoreFocus.label")}
        description={t("settings.advanced.restoreFocus.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  },
);
