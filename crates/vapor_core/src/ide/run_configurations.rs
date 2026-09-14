use super::xml_escape;
use std::path::{Path, PathBuf};

pub(super) fn build(
    project_root: &Path,
    installation_root: &Path,
) -> Vec<(PathBuf, String)> {
    #[cfg(windows)]
    {
        let _ = (project_root, installation_root);
        return Vec::new();
    }

    #[cfg(not(windows))]
    {
        let run_root = project_root.join(".run");
        let vapor = installation_root.join("bin/vapor");

        [
            ("Vapor_Installation_Diagnose.run.xml", "Vapor · Installation · Diagnose", "installation diagnose"),
            ("Vapor_Installation_Repair.run.xml", "Vapor · Installation · Repair", "installation repair"),
            ("Vapor_Client_Status.run.xml", "Vapor · Client · Status", "client status"),
            ("Vapor_Client_Build.run.xml", "Vapor · Client · Build", "client build"),
            ("Vapor_Client_Test.run.xml", "Vapor · Client · Test", "client test"),
            ("Vapor_Client_Deploy_Local.run.xml", "Vapor · Client · Deploy Local", "client deploy local"),
            ("Vapor_Platform_Server_Status.run.xml", "Vapor · Platform Server · Status", "platform-server status"),
            ("Vapor_Platform_Server_Runs.run.xml", "Vapor · Platform Server · Runs", "platform-server runs"),
            ("Vapor_Platform_Server_Build.run.xml", "Vapor · Platform Server · Build", "platform-server build"),
            ("Vapor_Platform_Server_Test.run.xml", "Vapor · Platform Server · Test", "platform-server test"),
            ("Vapor_Platform_Server_Deploy.run.xml", "Vapor · Platform Server · Deploy", "platform-server deploy"),
        ]
        .into_iter()
        .map(|(filename, name, arguments)| {
            (
                run_root.join(filename),
                shell_configuration(name, &vapor, arguments),
            )
        })
        .collect()
    }
}

#[cfg(not(windows))]
fn shell_configuration(name: &str, vapor: &Path, arguments: &str) -> String {
    let command = format!("{} {arguments}", shell_quote(&vapor.to_string_lossy()));

    format!(
        "<component name=\"ProjectRunConfigurationManager\">\n\
           <configuration default=\"false\" name=\"{}\" type=\"ShConfigurationType\">\n\
             <option name=\"SCRIPT_TEXT\" value=\"{}\" />\n\
             <option name=\"INDEPENDENT_SCRIPT_PATH\" value=\"true\" />\n\
             <option name=\"SCRIPT_PATH\" value=\"\" />\n\
             <option name=\"SCRIPT_OPTIONS\" value=\"\" />\n\
             <option name=\"INDEPENDENT_SCRIPT_WORKING_DIRECTORY\" value=\"true\" />\n\
             <option name=\"SCRIPT_WORKING_DIRECTORY\" value=\"$PROJECT_DIR$\" />\n\
             <option name=\"INDEPENDENT_INTERPRETER_PATH\" value=\"true\" />\n\
             <option name=\"INTERPRETER_PATH\" value=\"/bin/bash\" />\n\
             <option name=\"INTERPRETER_OPTIONS\" value=\"\" />\n\
             <option name=\"EXECUTE_IN_TERMINAL\" value=\"false\" />\n\
             <option name=\"EXECUTE_SCRIPT_FILE\" value=\"false\" />\n\
             <envs />\n\
             <method v=\"2\" />\n\
           </configuration>\n\
         </component>\n",
        xml_escape(name),
        xml_escape(&command),
    )
}

#[cfg(not(windows))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
