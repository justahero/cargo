//! cargo-sbom precursor files for external tools to create SBOM files from.
//! See [`output_sbom`] for more.

use std::collections::BTreeSet;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use cargo_util::paths::{self};
use cargo_util_schemas::core::PackageIdSpec;
use itertools::Itertools;
use serde::Serialize;

use crate::core::{profiles::Profile, PackageId};
use crate::core::{Target, TargetKind};
use crate::util::Rustc;
use crate::CargoResult;

use super::{unit_graph::UnitDep, BuildRunner, CrateType, Unit};

/// Typed version of a SBOM format version number.
pub struct SbomFormatVersion<const V: u32>;

impl<const V: u32> Serialize for SbomFormatVersion<V> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u32(V)
    }
}

/// A package dependency
#[derive(Serialize, Clone, Debug)]
struct SbomDependency {
    name: String,
    package_id: PackageId,
    version: String,
    features: Vec<String>,
}

impl From<&UnitDep> for SbomDependency {
    fn from(dep: &UnitDep) -> Self {
        let package_id = dep.unit.pkg.package_id();
        let name = package_id.name().to_string();
        let version = package_id.version().to_string();
        let features = dep
            .unit
            .features
            .iter()
            .map(|f| f.to_string())
            .collect_vec();

        Self {
            name,
            package_id,
            version,
            features,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
struct SbomPackage {
    package_id: PackageId,
    package: String,
    version: String,
    features: Vec<String>,
    extern_crate_name: String,
    dependencies: Vec<SbomDependency>,
}

impl SbomPackage {
    pub fn new(dep: &UnitDep, dependencies: Vec<SbomDependency>) -> Self {
        let package_id = dep.unit.pkg.package_id();
        let package = package_id.name().to_string();
        let version = package_id.version().to_string();
        let features = dep
            .unit
            .features
            .iter()
            .map(|f| f.to_string())
            .collect_vec();

        Self {
            package_id,
            package,
            version,
            features,
            extern_crate_name: dep.extern_crate_name.to_string(),
            dependencies,
        }
    }
}

#[derive(Serialize)]
struct SbomTarget {
    kind: TargetKind,
    crate_type: Option<CrateType>,
    name: String,
    edition: String,
}

impl From<&Target> for SbomTarget {
    fn from(target: &Target) -> Self {
        SbomTarget {
            kind: target.kind().clone(),
            crate_type: target.kind().rustc_crate_types().first().cloned(),
            name: target.name().to_string(),
            edition: target.edition().to_string(),
        }
    }
}

#[derive(Serialize, Clone)]
struct SbomRustc {
    version: String,
    wrapper: Option<PathBuf>,
    commit_hash: Option<String>,
    host: String,
}

impl From<&Rustc> for SbomRustc {
    fn from(rustc: &Rustc) -> Self {
        Self {
            version: rustc.version.to_string(),
            wrapper: rustc.wrapper.clone(),
            commit_hash: rustc.commit_hash.clone(),
            host: rustc.host.to_string(),
        }
    }
}

#[derive(Serialize)]
struct Sbom {
    format_version: SbomFormatVersion<1>,
    package_id: PackageIdSpec,
    name: String,
    version: String,
    source: String,
    target: SbomTarget,
    profile: Profile,
    packages: Vec<SbomPackage>,
    features: Vec<String>,
    rustc: SbomRustc,
}

impl Sbom {
    pub fn new(unit: &Unit, packages: Vec<SbomPackage>, rustc: SbomRustc) -> Self {
        let package_id = unit.pkg.summary().package_id().to_spec();
        let name = unit.pkg.name().to_string();
        let version = unit.pkg.version().to_string();
        let source = unit.pkg.package_id().source_id().to_string();
        let target = (&unit.target).into();
        let profile = unit.profile.clone();
        let features = unit.features.iter().map(|f| f.to_string()).collect();

        Self {
            format_version: SbomFormatVersion,
            package_id,
            name,
            version,
            source,
            target,
            profile,
            packages,
            features,
            rustc,
        }
    }
}

/// Saves a `<artifact>.cargo-sbom.json` file for the given [`Unit`].
///
pub fn output_sbom(build_runner: &mut BuildRunner<'_, '_>, unit: &Unit) -> CargoResult<()> {
    let bcx = build_runner.bcx;
    let rustc: SbomRustc = bcx.rustc().into();

    let packages = fetch_packages(build_runner, unit);

    for sbom_output_file in build_runner.sbom_output_files(unit)? {
        let sbom = Sbom::new(unit, packages.clone(), rustc.clone());

        let mut outfile = BufWriter::new(paths::create(sbom_output_file)?);
        let output = serde_json::to_string(&sbom)?;
        write!(outfile, "{}", output)?;
    }

    Ok(())
}

/// Fetch all dependencies, including transitive ones. A dependency can also appear multiple times
/// if it's included with different versions.
fn fetch_packages(build_runner: &mut BuildRunner<'_, '_>, unit: &Unit) -> Vec<SbomPackage> {
    let unit_graph = &build_runner.bcx.unit_graph;
    let root_deps = build_runner.unit_deps(unit);

    let mut result = Vec::new();
    let mut queue: BTreeSet<&UnitDep> = root_deps.iter().collect();
    let mut visited = BTreeSet::new();

    while let Some(package) = queue.pop_first() {
        // ignore any custom build scripts.
        if package.unit.mode.is_run_custom_build() {
            continue;
        }
        if visited.contains(package) {
            continue;
        }

        let mut dependencies: BTreeSet<&UnitDep> = unit_graph[&package.unit].iter().collect();
        let sbom_dependencies = dependencies.iter().map(|dep| (*dep).into()).collect_vec();

        result.push(SbomPackage::new(package, sbom_dependencies));

        visited.insert(package);

        queue.append(&mut dependencies);
    }

    result
}
