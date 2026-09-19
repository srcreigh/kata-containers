// Copyright (c) 2019 Ant Financial
//
// SPDX-License-Identifier: Apache-2.0
//

use oci_spec::runtime::Spec;

#[derive(Debug, Default, Clone)]
pub struct CreateOpts {
    pub spec: Option<Spec>,
}
