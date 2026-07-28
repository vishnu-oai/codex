use codex_config::HooksFile;

/// Parsed plugin metadata parameterized by its resource locator representation.
///
/// Host loading uses absolute paths, while resolved packages replace them with
/// authority-bound locators before exposing the manifest to consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest<Resource> {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub keywords: Vec<String>,
    pub setup: Option<PluginManifestSetup>,
    pub paths: PluginManifestPaths<Resource>,
    pub interface: Option<PluginManifestInterface<Resource>>,
}

/// Explicit foreground setup declared by a plugin package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifestSetup {
    /// User inputs collected once and exposed to every setup command.
    pub inputs: Vec<PluginManifestSetupInput>,
    /// Named commands executed in their declared order.
    pub commands: Vec<PluginManifestSetupCommand>,
}

/// One value that a plugin asks the foreground setup user to provide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifestSetupInput {
    pub id: String,
    pub input_type: PluginManifestSetupInputType,
    pub prompt: String,
    pub env: String,
    pub required: bool,
}

/// Validation and terminal-display behavior of a declared setup input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginManifestSetupInputType {
    Text,
    Directory,
    File,
    Secret,
}

/// One directly executed foreground setup step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifestSetupCommand {
    pub name: String,
    /// Executable followed by arguments; this is never evaluated as shell text.
    pub command: Vec<String>,
    /// Whether the approved process may read from the user's interactive terminal.
    pub interactive: bool,
}

/// Component resources declared by a plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifestPaths<Resource> {
    pub skills: Vec<Resource>,
    pub mcp_servers: Option<PluginManifestMcpServers<Resource>>,
    pub apps: Option<Resource>,
    pub hooks: Option<PluginManifestHooks<Resource>>,
}

/// MCP server declarations embedded in or referenced by a plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginManifestMcpServers<Resource> {
    Path(Resource),
    Object(String),
}

/// Hook declarations embedded in or referenced by a plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginManifestHooks<Resource> {
    Paths(Vec<Resource>),
    Inline(Vec<HooksFile>),
}

/// Optional model- and UI-facing plugin metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifestInterface<Resource> {
    pub display_name: Option<String>,
    pub short_description: Option<String>,
    pub long_description: Option<String>,
    pub developer_name: Option<String>,
    pub category: Option<String>,
    pub capabilities: Vec<String>,
    pub website_url: Option<String>,
    pub privacy_policy_url: Option<String>,
    pub terms_of_service_url: Option<String>,
    pub default_prompt: Option<Vec<String>>,
    pub brand_color: Option<String>,
    pub composer_icon: Option<Resource>,
    pub logo: Option<Resource>,
    pub logo_dark: Option<Resource>,
    pub screenshots: Vec<Resource>,
}

impl<Resource> Default for PluginManifestInterface<Resource> {
    fn default() -> Self {
        Self {
            display_name: None,
            short_description: None,
            long_description: None,
            developer_name: None,
            category: None,
            capabilities: Vec::new(),
            website_url: None,
            privacy_policy_url: None,
            terms_of_service_url: None,
            default_prompt: None,
            brand_color: None,
            composer_icon: None,
            logo: None,
            logo_dark: None,
            screenshots: Vec::new(),
        }
    }
}

impl<Resource> PluginManifest<Resource> {
    /// Returns the model- and UI-facing package name, falling back to the manifest name.
    pub fn display_name(&self) -> &str {
        self.interface
            .as_ref()
            .and_then(|interface| interface.display_name.as_deref())
            .map(str::trim)
            .filter(|display_name| !display_name.is_empty())
            .unwrap_or(&self.name)
    }

    /// Maps every path-bearing resource in the manifest.
    pub fn try_map_resources<Mapped, Error>(
        self,
        mut map: impl FnMut(Resource) -> Result<Mapped, Error>,
    ) -> Result<PluginManifest<Mapped>, Error> {
        let PluginManifest {
            name,
            version,
            description,
            keywords,
            setup,
            paths,
            interface,
        } = self;
        let PluginManifestPaths {
            skills,
            mcp_servers,
            apps,
            hooks,
        } = paths;
        let hooks = match hooks {
            Some(PluginManifestHooks::Paths(paths)) => Some(PluginManifestHooks::Paths(
                paths
                    .into_iter()
                    .map(&mut map)
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Some(PluginManifestHooks::Inline(hooks)) => Some(PluginManifestHooks::Inline(hooks)),
            None => None,
        };
        let mcp_servers = match mcp_servers {
            Some(PluginManifestMcpServers::Path(path)) => {
                Some(PluginManifestMcpServers::Path(map(path)?))
            }
            Some(PluginManifestMcpServers::Object(servers)) => {
                Some(PluginManifestMcpServers::Object(servers))
            }
            None => None,
        };
        let interface = match interface {
            Some(interface) => {
                let PluginManifestInterface {
                    display_name,
                    short_description,
                    long_description,
                    developer_name,
                    category,
                    capabilities,
                    website_url,
                    privacy_policy_url,
                    terms_of_service_url,
                    default_prompt,
                    brand_color,
                    composer_icon,
                    logo,
                    logo_dark,
                    screenshots,
                } = interface;
                Some(PluginManifestInterface {
                    display_name,
                    short_description,
                    long_description,
                    developer_name,
                    category,
                    capabilities,
                    website_url,
                    privacy_policy_url,
                    terms_of_service_url,
                    default_prompt,
                    brand_color,
                    composer_icon: composer_icon.map(&mut map).transpose()?,
                    logo: logo.map(&mut map).transpose()?,
                    logo_dark: logo_dark.map(&mut map).transpose()?,
                    screenshots: screenshots
                        .into_iter()
                        .map(&mut map)
                        .collect::<Result<Vec<_>, _>>()?,
                })
            }
            None => None,
        };

        Ok(PluginManifest {
            name,
            version,
            description,
            keywords,
            setup,
            paths: PluginManifestPaths {
                skills: skills
                    .into_iter()
                    .map(&mut map)
                    .collect::<Result<Vec<_>, _>>()?,
                mcp_servers,
                apps: apps.map(&mut map).transpose()?,
                hooks,
            },
            interface,
        })
    }
}
